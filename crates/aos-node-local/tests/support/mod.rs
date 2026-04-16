#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aos_air_types::{
    CapGrant, CapType, DefCap, DefEffect, DefModule, DefSchema, EmptyObject, HashRef, Manifest,
    ManifestDefaults, ModuleAbi, ModuleBinding, ModuleKind, Name, NamedRef, Routing, RoutingEvent,
    SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRecord, catalog::EffectCatalog,
};
use aos_cbor::Hash;
use aos_effect_types::TimerSetParams;
use aos_kernel::manifest::LoadedManifest;
use aos_kernel::{MemStore, Store, store_loaded_manifest};
use aos_node::{
    CreateWorldRequest, CreateWorldSource, FsCas, LocalControl, LocalStatePaths, WorldId,
};
use aos_wasm_abi::WorkflowOutput;
use indexmap::IndexMap;
use wat::parse_str;

pub mod fixtures {
    #[allow(unused_imports)]
    pub use super::{START_SCHEMA, start_event};
}

pub type TestStore = MemStore;

pub const START_SCHEMA: &str = "com.acme/Start@1";
pub const SYS_TIMER_FIRED: &str = "sys/TimerFired@1";

pub fn new_mem_store() -> Arc<TestStore> {
    Arc::new(MemStore::new())
}

pub fn start_event(id: &str) -> serde_json::Value {
    serde_json::json!({ "id": id })
}

pub fn create_simple_world(
    control: &Arc<LocalControl>,
    paths: &LocalStatePaths,
    world_id: WorldId,
    created_at_ns: u64,
) -> Result<aos_node::WorldCreateResult, Box<dyn std::error::Error>> {
    let manifest_hash = install_simple_manifest(paths)?;
    let created = control.create_world(CreateWorldRequest {
        world_id: Some(world_id),
        universe_id: aos_node::UniverseId::nil(),
        created_at_ns,
        source: CreateWorldSource::Manifest { manifest_hash },
    })?;
    Ok(created)
}

pub fn install_simple_manifest(
    paths: &LocalStatePaths,
) -> Result<String, Box<dyn std::error::Error>> {
    let cas = FsCas::open_with_paths(paths)?;
    let fixture_store = new_mem_store();
    let loaded = simple_state_manifest(&fixture_store);
    copy_manifest_module_blobs(&fixture_store, &cas, &loaded)?;
    let manifest_hash = store_loaded_manifest(&cas, &loaded)?;
    Ok(manifest_hash.to_hex())
}

pub fn install_timer_manifest(
    paths: &LocalStatePaths,
) -> Result<String, Box<dyn std::error::Error>> {
    let cas = FsCas::open_with_paths(paths)?;
    let fixture_store = new_mem_store();
    let loaded = timer_manifest(&fixture_store);
    copy_manifest_module_blobs(&fixture_store, &cas, &loaded)?;
    let manifest_hash = store_loaded_manifest(&cas, &loaded)?;
    Ok(manifest_hash.to_hex())
}

pub fn simple_state_manifest(store: &Arc<TestStore>) -> LoadedManifest {
    let mut workflow = stub_workflow_module(
        store,
        "com.acme/Simple@1",
        &WorkflowOutput {
            state: Some(vec![0xAA]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    workflow.abi.workflow = Some(aos_air_types::WorkflowAbi {
        state: schema("com.acme/SimpleState@1"),
        event: schema(START_SCHEMA),
        context: Some(schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest(
        vec![workflow],
        vec![routing_event(START_SCHEMA, "com.acme/Simple@1")],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            DefSchema {
                name: START_SCHEMA.into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("id".into(), text_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/SimpleState@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

pub fn timer_manifest(store: &Arc<TestStore>) -> LoadedManifest {
    let mut emitter = stub_workflow_module(
        store,
        "com.acme/TimerEmitter@1",
        &WorkflowOutput {
            state: Some(vec![0x01]),
            domain_events: vec![],
            effects: vec![aos_wasm_abi::WorkflowEffect::new(
                aos_effects::EffectKind::TIMER_SET,
                serde_cbor::to_vec(&TimerSetParams {
                    deliver_at_ns: 5,
                    key: Some("retry".into()),
                })
                .expect("timer params"),
            )],
            ann: None,
        },
    );
    emitter.abi.workflow = Some(aos_air_types::WorkflowAbi {
        state: schema("com.acme/TimerState@1"),
        event: schema(START_SCHEMA),
        context: Some(schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![aos_effects::EffectKind::TIMER_SET.into()],
        cap_slots: Default::default(),
    });

    let mut handler = stub_workflow_module(
        store,
        "com.acme/TimerHandler@1",
        &WorkflowOutput {
            state: Some(vec![0xCC]),
            domain_events: vec![],
            effects: vec![],
            ann: None,
        },
    );
    handler.abi.workflow = Some(aos_air_types::WorkflowAbi {
        state: schema("com.acme/TimerHandled@1"),
        event: schema(SYS_TIMER_FIRED),
        context: Some(schema("sys/WorkflowContext@1")),
        annotations: None,
        effects_emitted: vec![],
        cap_slots: Default::default(),
    });

    let mut loaded = build_loaded_manifest(
        vec![emitter, handler],
        vec![
            routing_event(START_SCHEMA, "com.acme/TimerEmitter@1"),
            routing_event(SYS_TIMER_FIRED, "com.acme/TimerHandler@1"),
        ],
    );
    insert_test_schemas(
        &mut loaded,
        vec![
            DefSchema {
                name: START_SCHEMA.into(),
                ty: TypeExpr::Record(TypeRecord {
                    record: IndexMap::from([("id".into(), text_type())]),
                }),
            },
            DefSchema {
                name: "com.acme/TimerState@1".into(),
                ty: text_type(),
            },
            DefSchema {
                name: "com.acme/TimerHandled@1".into(),
                ty: text_type(),
            },
        ],
    );
    loaded
}

fn copy_manifest_module_blobs(
    source: &Arc<TestStore>,
    target: &FsCas,
    loaded: &LoadedManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    for module in loaded.modules.values() {
        let hash = Hash::from_hex_str(module.wasm_hash.as_str())?;
        let bytes = source.get_blob(hash)?;
        let stored = target.put_blob(&bytes)?;
        assert_eq!(stored, hash, "copied wasm blob hash mismatch");
    }
    Ok(())
}

fn schema(name: &str) -> SchemaRef {
    SchemaRef::new(name).expect("schema ref")
}

fn zero_hash() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        .expect("zero hash ref")
}

fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject::default(),
    }))
}

fn insert_test_schemas(loaded: &mut LoadedManifest, schemas: Vec<DefSchema>) {
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

fn build_loaded_manifest(
    mut modules: Vec<DefModule>,
    routing_events: Vec<RoutingEvent>,
) -> LoadedManifest {
    let module_refs = modules
        .iter()
        .map(|module| {
            let def_hash =
                aos_cbor::Hash::of_cbor(&aos_air_types::AirNode::Defmodule(module.clone()))
                    .expect("hash defmodule");
            NamedRef {
                name: module.name.clone(),
                hash: HashRef::new(def_hash.to_hex()).expect("module hash ref"),
            }
        })
        .collect();

    let routing = if routing_events.is_empty() {
        None
    } else {
        Some(Routing {
            subscriptions: routing_events,
            inboxes: vec![],
        })
    };

    let mut manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: module_refs,
        effects: aos_air_types::builtins::builtin_effects()
            .iter()
            .map(|effect| NamedRef {
                name: effect.effect.name.clone(),
                hash: effect.hash_ref.clone(),
            })
            .collect(),
        effect_bindings: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing,
    };

    let modules_map: HashMap<Name, DefModule> = modules
        .drain(..)
        .map(|module| (module.name.clone(), module))
        .collect();
    let effects_map: HashMap<Name, DefEffect> = aos_air_types::builtins::builtin_effects()
        .iter()
        .map(|effect| (effect.effect.name.clone(), effect.effect.clone()))
        .collect();
    let effect_catalog = EffectCatalog::from_defs(effects_map.values().cloned());
    let caps = attach_test_capabilities(&mut manifest, modules_map.keys());

    let mut loaded = LoadedManifest {
        manifest,
        secrets: Vec::new(),
        modules: modules_map,
        effects: effects_map,
        caps,
        policies: HashMap::new(),
        schemas: HashMap::new(),
        effect_catalog,
    };
    ensure_placeholder_schemas(&mut loaded);
    loaded
}

fn attach_test_capabilities<'a, I>(manifest: &mut Manifest, modules: I) -> HashMap<Name, DefCap>
where
    I: IntoIterator<Item = &'a Name>,
{
    manifest.defaults = Some(ManifestDefaults {
        policy: None,
        cap_grants: vec![
            timer_cap_grant(),
            query_cap_grant(),
            http_cap_grant(),
            blob_cap_grant(),
        ],
    });
    manifest.caps = vec![
        NamedRef {
            name: "sys/http.out@1".into(),
            hash: zero_hash(),
        },
        NamedRef {
            name: "sys/timer@1".into(),
            hash: zero_hash(),
        },
        NamedRef {
            name: "sys/blob@1".into(),
            hash: zero_hash(),
        },
        NamedRef {
            name: "sys/query@1".into(),
            hash: zero_hash(),
        },
    ];

    let mut bindings = IndexMap::new();
    for module in modules {
        bindings.insert(
            module.clone(),
            ModuleBinding {
                slots: IndexMap::from([("default".into(), "timer_cap".into())]),
            },
        );
    }
    manifest.module_bindings = bindings;

    HashMap::from([
        ("sys/http.out@1".into(), http_defcap()),
        ("sys/timer@1".into(), timer_defcap()),
        ("sys/blob@1".into(), blob_defcap()),
        ("sys/query@1".into(), query_defcap()),
    ])
}

fn ensure_placeholder_schemas(loaded: &mut LoadedManifest) {
    let builtin_schema_map: HashMap<String, TypeExpr> = aos_air_types::builtins::builtin_schemas()
        .iter()
        .map(|builtin| (builtin.schema.name.clone(), builtin.schema.ty.clone()))
        .collect();
    let mut required = HashSet::from([START_SCHEMA.to_string()]);

    if let Some(routing) = &loaded.manifest.routing {
        for event in &routing.subscriptions {
            required.insert(event.event.as_str().to_string());
        }
    }
    for module in loaded.modules.values() {
        if let Some(workflow) = module.abi.workflow.as_ref() {
            required.insert(workflow.state.as_str().to_string());
            required.insert(workflow.event.as_str().to_string());
            for effect in &workflow.effects_emitted {
                if let Some(receipt_schema) = loaded.effect_catalog.receipt_schema(effect) {
                    required.insert(receipt_schema.as_str().to_string());
                }
            }
        }
    }

    for schema_name in required {
        if loaded.schemas.contains_key(&schema_name)
            || builtin_schema_map.contains_key(&schema_name)
        {
            continue;
        }
        let ty = if schema_name == START_SCHEMA {
            TypeExpr::Record(TypeRecord {
                record: IndexMap::from([("id".into(), text_type())]),
            })
        } else {
            TypeExpr::Record(TypeRecord {
                record: IndexMap::new(),
            })
        };
        loaded.schemas.insert(
            schema_name.clone(),
            DefSchema {
                name: schema_name.clone(),
                ty,
            },
        );
        if !loaded
            .manifest
            .schemas
            .iter()
            .any(|existing| existing.name == schema_name)
        {
            loaded.manifest.schemas.push(NamedRef {
                name: schema_name,
                hash: zero_hash(),
            });
        }
    }
}

fn stub_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    output: &WorkflowOutput,
) -> DefModule {
    let output_bytes = output.encode().expect("encode workflow output");
    let data_literal = output_bytes
        .iter()
        .map(|byte| format!("\\{:02x}", byte))
        .collect::<String>();
    let len = output_bytes.len();
    let wat = format!(
        r#"(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const {len}))
  (data (i32.const 0) "{data}")
  (func (export "alloc") (param i32) (result i32)
    (local $old i32)
    global.get $heap
    local.tee $old
    local.get 0
    i32.add
    global.set $heap
    local.get $old)
  (func (export "step") (param i32 i32) (result i32 i32)
    (i32.const 0)
    (i32.const {len}))
)"#,
        len = len,
        data = data_literal
    );
    let wasm_bytes = parse_str(&wat).expect("compile wat");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");

    DefModule {
        name: name.into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: HashRef::new(wasm_hash.to_hex()).expect("wasm hash ref"),
        key_schema: None,
        abi: ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}

fn routing_event(schema_name: &str, workflow: &str) -> RoutingEvent {
    RoutingEvent {
        event: schema(schema_name),
        module: workflow.to_string(),
        key_field: None,
    }
}

fn http_cap_grant() -> CapGrant {
    CapGrant {
        name: "cap_http".into(),
        cap: "sys/http.out@1".into(),
        params: empty_record_literal(),
        expiry_ns: None,
    }
}

fn timer_cap_grant() -> CapGrant {
    CapGrant {
        name: "timer_cap".into(),
        cap: "sys/timer@1".into(),
        params: empty_record_literal(),
        expiry_ns: None,
    }
}

fn blob_cap_grant() -> CapGrant {
    CapGrant {
        name: "blob_cap".into(),
        cap: "sys/blob@1".into(),
        params: empty_record_literal(),
        expiry_ns: None,
    }
}

fn query_cap_grant() -> CapGrant {
    CapGrant {
        name: "query_cap".into(),
        cap: "sys/query@1".into(),
        params: empty_record_literal(),
        expiry_ns: None,
    }
}

fn http_defcap() -> DefCap {
    DefCap {
        name: "sys/http.out@1".into(),
        cap_type: CapType::http_out(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapEnforceHttpOut@1".into(),
        },
    }
}

fn timer_defcap() -> DefCap {
    DefCap {
        name: "sys/timer@1".into(),
        cap_type: CapType::timer(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapAllowAll@1".into(),
        },
    }
}

fn blob_defcap() -> DefCap {
    DefCap {
        name: "sys/blob@1".into(),
        cap_type: CapType::blob(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapAllowAll@1".into(),
        },
    }
}

fn query_defcap() -> DefCap {
    DefCap {
        name: "sys/query@1".into(),
        cap_type: CapType::query(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapAllowAll@1".into(),
        },
    }
}

fn empty_record_literal() -> aos_air_types::ValueLiteral {
    aos_air_types::ValueLiteral::Record(aos_air_types::ValueRecord {
        record: IndexMap::new(),
    })
}
