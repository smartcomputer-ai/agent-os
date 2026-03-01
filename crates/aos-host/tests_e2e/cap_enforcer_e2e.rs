//! Integration tests for cap enforcer pure modules.
//!
//! These tests load the actual enforcer WASM built in `crates/aos-sys` from
//! `target/wasm32-unknown-unknown/debug`. Build it first with:
//! `cargo build -p aos-sys --target wasm32-unknown-unknown`.

#![cfg(feature = "e2e-tests")]

#[path = "../tests/helpers.rs"]
mod helpers;

use aos_air_types::{
    CapEnforcer, CapGrant, CapType, DefCap, EffectKind as AirEffectKind, EmptyObject, NamedRef,
    TypeExpr, TypeList, TypeOption, TypePrimitive, TypePrimitiveNat, TypePrimitiveText, TypeRecord,
    TypeSet, ValueList, ValueLiteral, ValueRecord, ValueSet, ValueText, WorkflowAbi,
};
use aos_effects::builtins::{
    HostFileContentInput, HostFsWriteFileParams, HostInlineText, HttpRequestParams,
    LlmGenerateParams, LlmRuntimeArgs,
};
use aos_wasm_abi::{WorkflowEffect, WorkflowOutput};
use helpers::fixtures::{self, TestWorld};
use indexmap::IndexMap;

#[test]
fn http_enforcer_module_denies_host() {
    let store = fixtures::new_mem_store();
    let workflow_name = "com.acme/HttpWorkflow@1";

    let output = WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HTTP_REQUEST,
            serde_cbor::to_vec(&HttpRequestParams {
                method: "GET".into(),
                url: "https://denied.example/path".into(),
                headers: IndexMap::new(),
                body_ref: None,
            })
            .unwrap(),
            "http",
        )],
        ann: None,
    };
    let mut workflow = fixtures::stub_workflow_module(&store, workflow_name, &output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/HttpState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HTTP_REQUEST.into()],
        cap_slots: Default::default(),
    });

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceHttpOut@1",
        "cap_enforce_http_out.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, enforcer],
        vec![fixtures::routing_event(
            fixtures::START_SCHEMA,
            workflow_name,
        )],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema("com.acme/HttpState@1", vec![]),
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
    loaded
        .manifest
        .module_bindings
        .get_mut(workflow_name)
        .expect("module binding")
        .slots
        .insert("http".into(), "cap_http".into());

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
    let workflow_name = "com.acme/LlmWorkflow@1";

    let output = WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::LLM_GENERATE,
            serde_cbor::to_vec(&LlmGenerateParams {
                correlation_id: None,
                provider: "openai".into(),
                model: "gpt-5.2".into(),
                message_refs: vec![fixtures::fake_hash(0x22)],
                runtime: LlmRuntimeArgs {
                    temperature: Some("0.4".into()),
                    top_p: None,
                    max_tokens: Some(10),
                    tool_refs: None,
                    tool_choice: None,
                    reasoning_effort: None,
                    stop_sequences: None,
                    metadata: None,
                    provider_options_ref: None,
                    response_format_ref: None,
                },
                api_key: None,
            })
            .unwrap(),
            "llm",
        )],
        ann: None,
    };
    let mut workflow = fixtures::stub_workflow_module(&store, workflow_name, &output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/LlmState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::LLM_GENERATE.into()],
        cap_slots: Default::default(),
    });

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceLlmBasic@1",
        "cap_enforce_llm_basic.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, enforcer],
        vec![fixtures::routing_event(
            fixtures::START_SCHEMA,
            workflow_name,
        )],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema("com.acme/LlmState@1", vec![]),
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
    loaded
        .manifest
        .module_bindings
        .get_mut(workflow_name)
        .expect("module binding")
        .slots
        .insert("llm".into(), "cap_llm".into());

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

#[test]
fn workspace_enforcer_module_denies_workspace() {
    let store = fixtures::new_mem_store();
    let workflow_name = "com.acme/WorkspaceWorkflow@1";

    let output = WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::WORKSPACE_RESOLVE,
            serde_cbor::to_vec(&serde_json::json!({
                "workspace": "alpha",
                "version": null
            }))
            .unwrap(),
            "workspace",
        )],
        ann: None,
    };
    let mut workflow = fixtures::stub_workflow_module(&store, workflow_name, &output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/WorkspaceState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::WORKSPACE_RESOLVE.into()],
        cap_slots: Default::default(),
    });

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceWorkspace@1",
        "cap_enforce_workspace.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, enforcer],
        vec![fixtures::routing_event(
            fixtures::START_SCHEMA,
            workflow_name,
        )],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema("com.acme/WorkspaceState@1", vec![]),
        ],
    );
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.push(CapGrant {
            name: "cap_workspace".into(),
            cap: "sys/workspace@1".into(),
            params: workspace_cap_params(&["beta"], &["resolve"]),
            expiry_ns: None,
        });
    }
    loaded.manifest.caps.push(NamedRef {
        name: "sys/workspace@1".into(),
        hash: fixtures::zero_hash(),
    });
    loaded
        .caps
        .insert("sys/workspace@1".into(), workspace_defcap());
    loaded
        .manifest
        .module_bindings
        .get_mut(workflow_name)
        .expect("module binding")
        .slots
        .insert("workspace".into(), "cap_workspace".into());

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
            } if cap_name == "cap_workspace" && kind == AirEffectKind::WORKSPACE_RESOLVE
        ),
        "unexpected error: {err:?}"
    );
}

#[test]
fn host_enforcer_module_denies_disallowed_fs_op() {
    let store = fixtures::new_mem_store();
    let workflow_name = "com.acme/HostWorkflow@1";

    let output = WorkflowOutput {
        state: None,
        domain_events: vec![],
        effects: vec![WorkflowEffect::with_cap_slot(
            aos_effects::EffectKind::HOST_FS_WRITE_FILE,
            serde_cbor::to_vec(&HostFsWriteFileParams {
                session_id: "sess-1".into(),
                path: "src/main.rs".into(),
                content: HostFileContentInput::InlineText {
                    inline_text: HostInlineText {
                        text: "fn main() {}\n".into(),
                    },
                },
                create_parents: Some(true),
                mode: Some("overwrite".into()),
            })
            .unwrap(),
            "host",
        )],
        ann: None,
    };
    let mut workflow = fixtures::stub_workflow_module(&store, workflow_name, &output);
    workflow.abi.workflow = Some(WorkflowAbi {
        state: fixtures::schema("com.acme/HostState@1"),
        event: fixtures::schema(fixtures::START_SCHEMA),
        context: Some(fixtures::schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::HOST_FS_WRITE_FILE.into()],
        cap_slots: Default::default(),
    });

    let enforcer = fixtures::pure_module_from_target(
        &store,
        "sys/CapEnforceHost@1",
        "cap_enforce_host.wasm",
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );

    let mut loaded = fixtures::build_loaded_manifest(
        vec![workflow, enforcer],
        vec![fixtures::routing_event(
            fixtures::START_SCHEMA,
            workflow_name,
        )],
    );
    fixtures::insert_test_schemas(
        &mut loaded,
        vec![
            helpers::def_text_record_schema(
                fixtures::START_SCHEMA,
                vec![("id", helpers::text_type())],
            ),
            helpers::def_text_record_schema("com.acme/HostState@1", vec![]),
        ],
    );
    if let Some(defaults) = loaded.manifest.defaults.as_mut() {
        defaults.cap_grants.push(CapGrant {
            name: "cap_host".into(),
            cap: "sys/host@1".into(),
            params: host_cap_params(&["read"], &["src/"]),
            expiry_ns: None,
        });
    }
    loaded.manifest.caps.push(NamedRef {
        name: "sys/host@1".into(),
        hash: fixtures::zero_hash(),
    });
    loaded.caps.insert("sys/host@1".into(), host_defcap());
    loaded
        .manifest
        .module_bindings
        .get_mut(workflow_name)
        .expect("module binding")
        .slots
        .insert("host".into(), "cap_host".into());

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
            } if cap_name == "cap_host" && kind == AirEffectKind::HOST_FS_WRITE_FILE
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

fn workspace_defcap() -> DefCap {
    DefCap {
        name: "sys/workspace@1".into(),
        cap_type: CapType::workspace(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([
                ("workspaces".into(), opt_text_list()),
                ("path_prefixes".into(), opt_text_list()),
                ("ops".into(), opt_text_set()),
            ]),
        }),
        enforcer: CapEnforcer {
            module: "sys/CapEnforceWorkspace@1".into(),
        },
    }
}

fn host_defcap() -> DefCap {
    DefCap {
        name: "sys/host@1".into(),
        cap_type: CapType::host(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([
                ("allowed_fs_ops".into(), opt_text_list()),
                ("fs_roots".into(), opt_text_list()),
                ("allowed_output_modes".into(), opt_text_list()),
            ]),
        }),
        enforcer: CapEnforcer {
            module: "sys/CapEnforceHost@1".into(),
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

fn workspace_cap_params(workspaces: &[&str], ops: &[&str]) -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "workspaces".into(),
                ValueLiteral::List(ValueList {
                    list: workspaces
                        .iter()
                        .map(|workspace| {
                            ValueLiteral::Text(ValueText {
                                text: (*workspace).to_string(),
                            })
                        })
                        .collect(),
                }),
            ),
            (
                "ops".into(),
                ValueLiteral::Set(ValueSet {
                    set: ops
                        .iter()
                        .map(|op| {
                            ValueLiteral::Text(ValueText {
                                text: (*op).to_string(),
                            })
                        })
                        .collect(),
                }),
            ),
        ]),
    })
}

fn host_cap_params(allowed_fs_ops: &[&str], fs_roots: &[&str]) -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::from([
            (
                "allowed_fs_ops".into(),
                ValueLiteral::List(ValueList {
                    list: allowed_fs_ops
                        .iter()
                        .map(|op| {
                            ValueLiteral::Text(ValueText {
                                text: (*op).to_string(),
                            })
                        })
                        .collect(),
                }),
            ),
            (
                "fs_roots".into(),
                ValueLiteral::List(ValueList {
                    list: fs_roots
                        .iter()
                        .map(|root| {
                            ValueLiteral::Text(ValueText {
                                text: (*root).to_string(),
                            })
                        })
                        .collect(),
                }),
            ),
            (
                "allowed_output_modes".into(),
                ValueLiteral::List(ValueList {
                    list: vec![ValueLiteral::Text(ValueText {
                        text: "auto".into(),
                    })],
                }),
            ),
        ]),
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

fn opt_text_list() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(TypeExpr::List(TypeList {
            list: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                TypePrimitiveText {
                    text: EmptyObject {},
                },
            ))),
        })),
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
