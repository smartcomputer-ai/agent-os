#![cfg(feature = "test-fixtures")]

//! Integration tests for cap enforcer pure modules.
//!
//! These tests load the actual enforcer WASM built in `crates/aos-sys` from
//! `target/wasm32-unknown-unknown/debug`. Build it first with:
//! `cargo build -p aos-sys --target wasm32-unknown-unknown`.

#[path = "helpers.rs"]
mod helpers;

use aos_air_types::{
    CapEnforcer, CapGrant, CapType, DefCap, DefPlan, EffectKind as AirEffectKind, EmptyObject,
    ExprOrValue, NamedRef, PlanBindEffect, PlanEdge, PlanStep, PlanStepEmitEffect, PlanStepEnd,
    PlanStepKind, TypeExpr, TypeOption, TypePrimitive, TypePrimitiveNat, TypePrimitiveText,
    TypeRecord, TypeSet, ValueLiteral, ValueMap, ValueNull, ValueRecord, ValueSet, ValueText,
};
use helpers::fixtures::{self, TestWorld};
use indexmap::IndexMap;

#[test]
fn http_enforcer_module_denies_host() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/HttpPlan@1".to_string();
    let plan = http_plan(&plan_name, "cap_http", "https://denied.example/path");

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceHttpOut@1",
        "cap_enforce_http_out.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![enforcer],
        vec![],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema(
                "com.acme/HttpPlanIn@1",
                vec![("id", helpers::text_type())],
            ),
        ],
    );

    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        for grant in &mut defaults.cap_grants {
            if grant.name == "cap_http" {
                grant.params = hosts_param(&["example.com"]);
            }
        }
    }
    loaded.caps.insert("sys/http.out@1".into(), http_defcap());

    let mut world = TestWorld::with_store(store, loaded).expect("world");
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    let err = world.kernel.tick().expect_err("expected denial");
    assert!(
        matches!(
            err,
            aos_kernel::error::KernelError::CapabilityDenied {
                cap: ref cap_name,
                effect_kind: ref kind,
                ..
            } if cap_name == "cap_http" && kind == AirEffectKind::HTTP_REQUEST
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn llm_enforcer_module_denies_model() {
    let store = fixtures::new_mem_store();
    let plan_name = "com.acme/LlmPlan@1".to_string();
    let plan = llm_plan(&plan_name, "cap_llm", "openai", "gpt-4", 10);

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceLlmBasic@1",
        "cap_enforce_llm_basic.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![plan],
        vec![fixtures::start_trigger(&plan_name)],
        vec![enforcer],
        vec![],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema(
                "com.acme/LlmPlanIn@1",
                vec![("id", helpers::text_type())],
            ),
        ],
    );

    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.push(CapGrant {
            name: "cap_llm".into(),
            cap: "sys/llm.basic@1".into(),
            params: models_param(&["gpt-3.5"]),
            expiry_ns: None,
        });
    }
    loaded.manifest.caps.push(NamedRef {
        name: "sys/llm.basic@1".into(),
        hash: fixtures::zero_hash(),
    });
    loaded.caps.insert("sys/llm.basic@1".into(), llm_defcap());

    let mut world = TestWorld::with_store(store, loaded).expect("world");
    world
        .submit_event_result(
            fixtures::START_SCHEMA,
            &serde_json::json!({ "id": "start" }),
        )
        .expect("submit start event");
    let err = world.kernel.tick().expect_err("expected denial");
    assert!(
        matches!(
            err,
            aos_kernel::error::KernelError::CapabilityDenied {
                cap: ref cap_name,
                effect_kind: ref kind,
                ..
            } if cap_name == "cap_llm" && kind == AirEffectKind::LLM_GENERATE
        ),
        "unexpected error: {err:?}"
    );
}

fn http_defcap() -> DefCap {
    DefCap {
        name: "sys/http.out@1".into(),
        cap_type: CapType::http_out(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([
                ("hosts".into(), opt_text_set()),
                ("schemes".into(), opt_text_set()),
                ("methods".into(), opt_text_set()),
                ("ports".into(), opt_nat_set()),
                ("path_prefixes".into(), opt_text_set()),
            ]),
        }),
        enforcer: CapEnforcer {
            module: "sys/CapEnforceHttpOut@1".into(),
        },
    }
}

fn llm_defcap() -> DefCap {
    DefCap {
        name: "sys/llm.basic@1".into(),
        cap_type: CapType::llm_basic(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([
                ("providers".into(), opt_text_set()),
                ("models".into(), opt_text_set()),
                ("max_tokens".into(), opt_nat()),
                ("tools_allow".into(), opt_text_set()),
            ]),
        }),
        enforcer: CapEnforcer {
            module: "sys/CapEnforceLlmBasic@1".into(),
        },
    }
}

fn hosts_param(hosts: &[&str]) -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([(
            "hosts".into(),
            ValueLiteral::Set(ValueSet {
                set: hosts
                    .iter()
                    .map(|host| {
                        ValueLiteral::Text(ValueText {
                            text: (*host).to_string(),
                        })
                    })
                    .collect(),
            }),
        )]),
    })
}

fn models_param(models: &[&str]) -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([(
            "models".into(),
            ValueLiteral::Set(ValueSet {
                set: models
                    .iter()
                    .map(|model| {
                        ValueLiteral::Text(ValueText {
                            text: (*model).to_string(),
                        })
                    })
                    .collect(),
            }),
        )]),
    })
}

fn opt_text_set() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                TypePrimitiveText {
                    text: EmptyObject {},
                },
            ))),
        })),
    })
}

fn opt_nat_set() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(TypeExpr::Set(TypeSet {
            set: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
                nat: EmptyObject {},
            }))),
        })),
    })
}

fn opt_nat() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
            nat: EmptyObject {},
        }))),
    })
}

fn http_plan(plan_name: &str, cap: &str, url: &str) -> DefPlan {
    DefPlan {
        name: plan_name.to_string(),
        input: fixtures::schema("com.acme/HttpPlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: AirEffectKind::http_request(),
                    params: http_params_literal(url),
                    cap: cap.to_string(),
                    idempotency_key: None,
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![PlanEdge {
            from: "emit".into(),
            to: "end".into(),
            when: None,
        }],
        required_caps: vec![cap.to_string()],
        allowed_effects: vec![AirEffectKind::http_request()],
        invariants: vec![],
    }
}

fn llm_plan(plan_name: &str, cap: &str, provider: &str, model: &str, max_tokens: u64) -> DefPlan {
    DefPlan {
        name: plan_name.to_string(),
        input: fixtures::schema("com.acme/LlmPlanIn@1"),
        output: None,
        locals: IndexMap::new(),
        steps: vec![
            PlanStep {
                id: "emit".into(),
                kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                    kind: AirEffectKind::llm_generate(),
                    params: llm_params_literal(provider, model, max_tokens),
                    cap: cap.to_string(),
                    idempotency_key: None,
                    bind: PlanBindEffect {
                        effect_id_as: "req".into(),
                    },
                }),
            },
            PlanStep {
                id: "end".into(),
                kind: PlanStepKind::End(PlanStepEnd { result: None }),
            },
        ],
        edges: vec![PlanEdge {
            from: "emit".into(),
            to: "end".into(),
            when: None,
        }],
        required_caps: vec![cap.to_string()],
        allowed_effects: vec![AirEffectKind::llm_generate()],
        invariants: vec![],
    }
}

fn http_params_literal(url: &str) -> ExprOrValue {
    ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "method".into(),
                ValueLiteral::Text(ValueText { text: "GET".into() }),
            ),
            (
                "url".into(),
                ValueLiteral::Text(ValueText { text: url.into() }),
            ),
            (
                "headers".into(),
                ValueLiteral::Map(ValueMap { map: vec![] }),
            ),
            (
                "body_ref".into(),
                ValueLiteral::Null(ValueNull {
                    null: EmptyObject::default(),
                }),
            ),
        ]),
    }))
}

fn llm_params_literal(provider: &str, model: &str, max_tokens: u64) -> ExprOrValue {
    ExprOrValue::Literal(ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "provider".into(),
                ValueLiteral::Text(ValueText {
                    text: provider.to_string(),
                }),
            ),
            (
                "model".into(),
                ValueLiteral::Text(ValueText {
                    text: model.to_string(),
                }),
            ),
            (
                "temperature".into(),
                ValueLiteral::Dec128(aos_air_types::ValueDec128 {
                    dec128: "0.4".into(),
                }),
            ),
            (
                "max_tokens".into(),
                ValueLiteral::Nat(aos_air_types::ValueNat { nat: max_tokens }),
            ),
            (
                "input_ref".into(),
                ValueLiteral::Hash(aos_air_types::ValueHash {
                    hash: fixtures::fake_hash(0x11),
                }),
            ),
            (
                "tools".into(),
                ValueLiteral::Null(ValueNull {
                    null: EmptyObject::default(),
                }),
            ),
            (
                "api_key".into(),
                ValueLiteral::Null(ValueNull {
                    null: EmptyObject::default(),
                }),
            ),
        ]),
    }))
}
