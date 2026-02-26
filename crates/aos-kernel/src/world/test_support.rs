use super::*;
use crate::journal::{JournalEntry, JournalKind, mem::MemJournal};
use aos_air_types::{
    CURRENT_AIR_VERSION, DefSchema, HashRef, ModuleAbi, ModuleKind, NamedRef, ReducerAbi,
    Routing, RoutingEvent, SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveText, TypeRecord,
    catalog::EffectCatalog,
};
use aos_store::MemStore;
use indexmap::IndexMap;
use serde_cbor::Value as CborValue;
use serde_cbor::ser::to_vec;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Write;
use std::sync::Arc;

pub(crate) fn named_ref(name: &str, hash: &str) -> NamedRef {
    NamedRef {
        name: name.into(),
        hash: HashRef::new(hash).unwrap(),
    }
}

pub(crate) fn hash(num: u64) -> String {
    format!("sha256:{num:064x}")
}

pub(crate) fn minimal_manifest() -> Manifest {
    Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![],
        caps: vec![],
        effects: vec![],
        policies: vec![],
        secrets: vec![],
        module_bindings: IndexMap::new(),
        routing: None,
        defaults: None,
    }
}

pub(crate) fn dummy_stamp<S: Store + 'static>(kernel: &Kernel<S>) -> IngressStamp {
    IngressStamp {
        now_ns: 0,
        logical_now_ns: 0,
        entropy: vec![0u8; ENTROPY_LEN],
        journal_height: 0,
        manifest_hash: kernel.manifest_hash().to_hex(),
    }
}

pub(crate) fn schema_text(name: &str) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
            text: Default::default(),
        })),
    }
}

pub(crate) fn schema_event_record(name: &str) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "id".into(),
                TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                    text: Default::default(),
                })),
            )]),
        }),
    }
}

pub(crate) fn loaded_manifest_with_schema(
    store: &MemStore,
    schema_name: &str,
) -> (LoadedManifest, aos_cbor::Hash) {
    let schema = schema_event_record(schema_name);
    let schema_hash = store
        .put_node(&AirNode::Defschema(schema.clone()))
        .expect("store schema");
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![NamedRef {
            name: schema_name.into(),
            hash: HashRef::new(schema_hash.to_hex()).unwrap(),
        }],
        modules: vec![],
        caps: vec![],
        effects: vec![],
        policies: vec![],
        secrets: vec![],
        module_bindings: Default::default(),
        routing: None,
        defaults: None,
    };
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules: HashMap::new(),
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas: HashMap::from([(schema_name.into(), schema)]),
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    let mut loaded = loaded;
    manifest_runtime::persist_loaded_manifest(store, &mut loaded).expect("persist manifest");
    let manifest_hash = store.put_node(&loaded.manifest).expect("store manifest");
    (loaded, manifest_hash)
}

pub(crate) fn event_record(schema: &str, journal_height: u64) -> DomainEventRecord {
    let payload = serde_cbor::to_vec(&CborValue::Map(BTreeMap::from([(
        CborValue::Text("id".into()),
        CborValue::Text("1".into()),
    )])))
    .unwrap();
    DomainEventRecord {
        schema: schema.to_string(),
        value: payload,
        key: None,
        now_ns: 0,
        logical_now_ns: 0,
        journal_height,
        entropy: vec![0u8; ENTROPY_LEN],
        event_hash: String::new(),
        manifest_hash: String::new(),
    }
}

pub(crate) fn append_record(journal: &mut MemJournal, record: JournalRecord) {
    let bytes = serde_cbor::to_vec(&record).expect("encode record");
    journal
        .append(JournalEntry::new(record.kind(), &bytes))
        .expect("append record");
}

pub(crate) fn minimal_kernel_with_router() -> Kernel<aos_store::MemStore> {
    let store = aos_store::MemStore::default();
    let module = DefModule {
        name: "com.acme/Reducer@1".into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: HashRef::new(hash(1)).unwrap(),
        key_schema: Some(SchemaRef::new("com.acme/Key@1").unwrap()),
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: SchemaRef::new("com.acme/State@1").unwrap(),
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                annotations: None,
                effects_emitted: vec![],
                cap_slots: Default::default(),
            }),
            pure: None,
        },
    };
    let mut modules = HashMap::new();
    modules.insert(module.name.clone(), module);
    let mut schemas = HashMap::new();
    schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
    schemas.insert(
        "com.acme/Event@1".into(),
        schema_event_record("com.acme/Event@1"),
    );
    schemas.insert("com.acme/Key@1".into(), schema_text("com.acme/Key@1"));
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![NamedRef {
            name: "com.acme/Reducer@1".into(),
            hash: HashRef::new(hash(1)).unwrap(),
        }],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                module: "com.acme/Reducer@1".to_string(),
                key_field: Some("id".into()),
            }],
            inboxes: vec![],
        }),
    };
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules,
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    Kernel::from_loaded_manifest(
        Arc::new(store),
        loaded,
        Box::new(crate::journal::mem::MemJournal::default()),
    )
    .unwrap()
}

pub(crate) fn minimal_kernel_with_router_non_keyed() -> Kernel<aos_store::MemStore> {
    let store = aos_store::MemStore::default();
    let module = DefModule {
        name: "com.acme/Reducer@1".into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: HashRef::new(hash(1)).unwrap(),
        key_schema: None,
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: SchemaRef::new("com.acme/State@1").unwrap(),
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                annotations: None,
                effects_emitted: vec![],
                cap_slots: Default::default(),
            }),
            pure: None,
        },
    };
    let mut modules = HashMap::new();
    modules.insert(module.name.clone(), module);
    let mut schemas = HashMap::new();
    schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
    schemas.insert(
        "com.acme/Event@1".into(),
        schema_event_record("com.acme/Event@1"),
    );
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![NamedRef {
            name: "com.acme/Reducer@1".into(),
            hash: HashRef::new(hash(1)).unwrap(),
        }],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                module: "com.acme/Reducer@1".to_string(),
                key_field: Some("id".into()),
            }],
            inboxes: vec![],
        }),
    };
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules,
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    Kernel::from_loaded_manifest(
        Arc::new(store),
        loaded,
        Box::new(crate::journal::mem::MemJournal::default()),
    )
    .unwrap()
}

pub(crate) fn minimal_kernel_non_keyed() -> Kernel<aos_store::MemStore> {
    let store = aos_store::MemStore::default();
    let module = DefModule {
        name: "com.acme/Reducer@1".into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: HashRef::new(hash(1)).unwrap(),
        key_schema: None,
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: SchemaRef::new("com.acme/State@1").unwrap(),
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                annotations: None,
                effects_emitted: vec![],
                cap_slots: Default::default(),
            }),
            pure: None,
        },
    };
    let mut modules = HashMap::new();
    modules.insert(module.name.clone(), module);
    let mut schemas = HashMap::new();
    schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
    schemas.insert(
        "com.acme/Event@1".into(),
        schema_event_record("com.acme/Event@1"),
    );
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![NamedRef {
            name: "com.acme/Reducer@1".into(),
            hash: HashRef::new(hash(1)).unwrap(),
        }],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                module: "com.acme/Reducer@1".to_string(),
                key_field: None,
            }],
            inboxes: vec![],
        }),
    };
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules,
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    Kernel::from_loaded_manifest(
        Arc::new(store),
        loaded,
        Box::new(crate::journal::mem::MemJournal::default()),
    )
    .unwrap()
}

pub(crate) fn minimal_kernel_keyed_missing_key_field() -> Kernel<aos_store::MemStore> {
    let store = aos_store::MemStore::default();
    let module = DefModule {
        name: "com.acme/Reducer@1".into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: HashRef::new(hash(1)).unwrap(),
        key_schema: Some(SchemaRef::new("com.acme/Key@1").unwrap()),
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: SchemaRef::new("com.acme/State@1").unwrap(),
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                context: Some(SchemaRef::new("sys/ReducerContext@1").unwrap()),
                annotations: None,
                effects_emitted: vec![],
                cap_slots: Default::default(),
            }),
            pure: None,
        },
    };
    let mut modules = HashMap::new();
    modules.insert(module.name.clone(), module);
    let mut schemas = HashMap::new();
    schemas.insert("com.acme/State@1".into(), schema_text("com.acme/State@1"));
    schemas.insert(
        "com.acme/Event@1".into(),
        schema_event_record("com.acme/Event@1"),
    );
    schemas.insert("com.acme/Key@1".into(), schema_text("com.acme/Key@1"));
    let manifest = Manifest {
        air_version: CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![NamedRef {
            name: "com.acme/Reducer@1".into(),
            hash: HashRef::new(hash(1)).unwrap(),
        }],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing: Some(Routing {
            subscriptions: vec![RoutingEvent {
                event: SchemaRef::new("com.acme/Event@1").unwrap(),
                module: "com.acme/Reducer@1".to_string(),
                key_field: None,
            }],
            inboxes: vec![],
        }),
    };
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules,
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    Kernel::from_loaded_manifest(
        Arc::new(store),
        loaded,
        Box::new(crate::journal::mem::MemJournal::default()),
    )
    .unwrap()
}

pub(crate) fn empty_manifest() -> Manifest {
    Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: vec![],
        modules: vec![],
        effects: vec![],
        caps: vec![],
        policies: vec![],
        secrets: vec![],
        defaults: None,
        module_bindings: Default::default(),
        routing: None,
    }
}

pub(crate) fn write_manifest(path: &std::path::Path, manifest: &Manifest) {
    let bytes = to_vec(manifest).expect("serialize manifest");
    let mut file = File::create(path).expect("create manifest file");
    file.write_all(&bytes).expect("write manifest");
}

pub(crate) fn kernel_with_store_and_journal(
    store: Arc<MemStore>,
    journal: Box<dyn Journal>,
) -> Kernel<MemStore> {
    let manifest = empty_manifest();
    let loaded = LoadedManifest {
        manifest,
        secrets: vec![],
        modules: HashMap::new(),
        effects: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas: HashMap::new(),
        effect_catalog: EffectCatalog::from_defs(Vec::new()),
    };
    Kernel::from_loaded_manifest_with_config(store, loaded, journal, KernelConfig::default())
        .expect("build kernel")
}
