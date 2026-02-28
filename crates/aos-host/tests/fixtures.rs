//! Test fixtures for building manifests, stub workflows, and test data.
//!
//! This module provides utilities for programmatically constructing manifests,
//! stub WASM workflows, and other test fixtures. Enable with the `e2e-tests` feature.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use aos_air_exec::{Value as ExprValue, ValueKey as ExprValueKey};
use aos_air_types::{
    CapGrant, CapType, DefCap, DefEffect, DefModule, DefSchema, EmptyObject, HashRef, Manifest,
    ManifestDefaults, ModuleAbi, ModuleBinding, ModuleKind, Name, NamedRef, OriginScope, Routing,
    RoutingEvent, SchemaRef, TypeExpr, TypeOption, TypePrimitive, TypePrimitiveText, TypeRecord,
    ValueLiteral, ValueRecord, catalog::EffectCatalog,
};
use aos_cbor::Hash;
use aos_kernel::manifest::LoadedManifest;
use aos_store::{MemStore, Store};
use aos_wasm_abi::{DomainEvent, PureOutput, WorkflowOutput};
use indexmap::IndexMap;
use std::fs;
use std::path::PathBuf;
use wat::parse_str;

/// In-memory store alias used across fixtures.
pub type TestStore = MemStore;

/// Standard start schema used for triggering workflows in tests.
pub const START_SCHEMA: &str = "com.acme/Start@1";

/// Built-in timer fired schema.
pub const SYS_TIMER_FIRED: &str = "sys/TimerFired@1";

/// Returns a schema reference for reuse in manifests and workflow fixtures.
pub fn schema(name: &str) -> SchemaRef {
    SchemaRef::new(name).unwrap()
}

/// Build a canonical start event payload matching the common Start schema
/// (record with required `id: text` field).
pub fn start_event(id: &str) -> serde_json::Value {
    serde_json::json!({ "id": id })
}

/// Returns the zero hash helper used as a placeholder for manifest references.
pub fn zero_hash() -> HashRef {
    HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000").unwrap()
}

/// Returns a fake hash reference with all bytes set to the provided value.
/// Useful for creating placeholder hashes in tests without needing actual content.
pub fn fake_hash(byte: u8) -> HashRef {
    let hex = format!("{:02x}", byte);
    HashRef::new(format!("sha256:{}", hex.repeat(32))).unwrap()
}

/// Convenience: text primitive TypeExpr.
pub fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: EmptyObject::default(),
    }))
}

/// Convenience: defschema for a text record with explicit fields.
pub fn def_text_record_schema(name: &str, fields: Vec<(&str, TypeExpr)>) -> DefSchema {
    DefSchema {
        name: name.into(),
        ty: TypeExpr::Record(TypeRecord {
            record: IndexMap::from_iter(fields.into_iter().map(|(k, v)| (k.to_string(), v))),
        }),
    }
}

/// Insert schemas into LoadedManifest (both map and manifest.schemas NamedRefs).
pub fn insert_test_schemas(loaded: &mut LoadedManifest, schemas: Vec<DefSchema>) {
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

/// Builds a `LoadedManifest` from workflow/pure modules and routing subscriptions.
pub fn build_loaded_manifest(
    mut modules: Vec<DefModule>,
    routing_events: Vec<RoutingEvent>,
) -> LoadedManifest {
    let module_refs: Vec<NamedRef> = modules
        .iter()
        .map(|module| {
            let def_hash =
                aos_cbor::Hash::of_cbor(&aos_air_types::AirNode::Defmodule(module.clone()))
                    .expect("hash defmodule");
            NamedRef {
                name: module.name.clone(),
                hash: HashRef::new(def_hash.to_hex()).expect("hash ref"),
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
            .map(|e| NamedRef {
                name: e.effect.name.clone(),
                hash: e.hash_ref.clone(),
            })
            .collect(),
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
        .map(|e| (e.effect.name.clone(), e.effect.clone()))
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

/// Populates the manifest with default capability grants and module slot bindings so workflows
/// can emit timer/blob effects without extra ceremony.
pub fn attach_test_capabilities<'a, I>(manifest: &mut Manifest, modules: I) -> HashMap<Name, DefCap>
where
    I: IntoIterator<Item = &'a Name>,
{
    manifest.defaults = Some(ManifestDefaults {
        policy: None,
        cap_grants: vec![
            cap_http_grant(),
            timer_cap_grant(),
            blob_cap_grant(),
            query_cap_grant(),
        ],
    });
    // Ensure manifest declares the capabilities we grant.
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
    let mut required: HashSet<String> = HashSet::new();
    required.insert(START_SCHEMA.to_string());

    let builtin_schema_map: HashMap<String, TypeExpr> = aos_air_types::builtins::builtin_schemas()
        .iter()
        .map(|builtin| (builtin.schema.name.clone(), builtin.schema.ty.clone()))
        .collect();

    let schema_type = |name: &str, loaded: &LoadedManifest| -> Option<TypeExpr> {
        loaded
            .schemas
            .get(name)
            .map(|def| def.ty.clone())
            .or_else(|| builtin_schema_map.get(name).cloned())
    };

    if let Some(routing) = &loaded.manifest.routing {
        for event in &routing.subscriptions {
            required.insert(event.event.as_str().to_string());
        }
    }
    for module in loaded.modules.values() {
        if let Some(workflow) = module.abi.workflow.as_ref() {
            required.insert(workflow.state.as_str().to_string());
            required.insert(workflow.event.as_str().to_string());
            if let Some(event_schema) = schema_type(workflow.event.as_str(), loaded) {
                match event_schema {
                    TypeExpr::Ref(reference) => {
                        required.insert(reference.reference.as_str().to_string());
                    }
                    TypeExpr::Variant(variant) => {
                        for member in variant.variant.values() {
                            if let TypeExpr::Ref(reference) = member {
                                required.insert(reference.reference.as_str().to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            for effect in &workflow.effects_emitted {
                if let Some(receipt_schema) = loaded.effect_catalog.receipt_schema(effect) {
                    required.insert(receipt_schema.as_str().to_string());
                }
            }
        }
        if let Some(key_schema) = &module.key_schema {
            required.insert(key_schema.as_str().to_string());
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
                record: IndexMap::from([(
                    "id".into(),
                    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                        text: EmptyObject {},
                    })),
                )]),
            })
        } else {
            TypeExpr::Record(TypeRecord {
                record: IndexMap::new(),
            })
        };
        let def = DefSchema {
            name: schema_name.clone(),
            ty,
        };
        loaded.schemas.insert(schema_name.clone(), def);
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

/// HTTP capability grant for tests.
pub fn cap_http_grant() -> CapGrant {
    CapGrant {
        name: "cap_http".into(),
        cap: "sys/http.out@1".into(),
        params: empty_value_literal(),
        expiry_ns: None,
    }
}

/// Timer capability grant for tests.
pub fn timer_cap_grant() -> CapGrant {
    CapGrant {
        name: "timer_cap".into(),
        cap: "sys/timer@1".into(),
        params: empty_value_literal(),
        expiry_ns: None,
    }
}

/// Blob capability grant for tests.
pub fn blob_cap_grant() -> CapGrant {
    CapGrant {
        name: "blob_cap".into(),
        cap: "sys/blob@1".into(),
        params: empty_value_literal(),
        expiry_ns: None,
    }
}

/// Query capability grant for tests (introspection).
pub fn query_cap_grant() -> CapGrant {
    CapGrant {
        name: "query_cap".into(),
        cap: "sys/query@1".into(),
        params: empty_value_literal(),
        expiry_ns: None,
    }
}

/// Minimal HTTP capability definition used inside tests.
pub fn http_defcap() -> DefCap {
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

/// Minimal Timer capability definition used inside tests.
pub fn timer_defcap() -> DefCap {
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

/// Minimal Blob capability definition used inside tests.
pub fn blob_defcap() -> DefCap {
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

/// Minimal Query capability definition used inside tests (introspection).
pub fn query_defcap() -> DefCap {
    DefCap {
        name: "sys/query@1".into(),
        cap_type: CapType::query(),
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "scope".into(),
                TypeExpr::Option(TypeOption {
                    option: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                        TypePrimitiveText {
                            text: EmptyObject {},
                        },
                    ))),
                }),
            )]),
        }),
        enforcer: aos_air_types::CapEnforcer {
            module: "sys/CapAllowAll@1".into(),
        },
    }
}

/// Handy empty record literal for cap grant params.
pub fn empty_value_literal() -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::new(),
    })
}

/// Construct a fresh in-memory store for use in tests.
pub fn new_mem_store() -> Arc<TestStore> {
    Arc::new(MemStore::new())
}

/// Compiles a trivial WAT module whose `step` export always returns the provided
/// `WorkflowOutput` bytes. Useful for workflows that simply emit domain events or effects.
pub fn stub_workflow_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    output: &WorkflowOutput,
) -> DefModule {
    let output_bytes = output.encode().expect("encode workflow output");
    let data_literal = output_bytes
        .iter()
        .map(|b| format!("\\{:02x}", b))
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
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");

    DefModule {
        name: name.into(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: wasm_hash_ref,
        key_schema: None,
        abi: ModuleAbi {
            workflow: None,
            pure: None,
        },
    }
}

/// Compiles a trivial WAT module whose `run` export always returns the provided
/// `PureOutput` bytes. Useful for exercising kernel pure-module dispatch.
pub fn stub_pure_module<S: Store + ?Sized>(
    store: &Arc<S>,
    name: impl Into<String>,
    output: &PureOutput,
    input_schema: &str,
    output_schema: &str,
) -> DefModule {
    let output_bytes = output.encode().expect("encode pure output");
    let data_literal = output_bytes
        .iter()
        .map(|b| format!("\\{:02x}", b))
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
  (func (export "run") (param i32 i32) (result i32 i32)
    (i32.const 0)
    (i32.const {len}))
)"#,
        len = len,
        data = data_literal
    );
    let wasm_bytes = parse_str(&wat).expect("wat compile");
    let wasm_hash = store.put_blob(&wasm_bytes).expect("store wasm");
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");

    DefModule {
        name: name.into(),
        module_kind: ModuleKind::Pure,
        wasm_hash: wasm_hash_ref,
        key_schema: None,
        abi: ModuleAbi {
            workflow: None,
            pure: Some(aos_air_types::PureAbi {
                input: schema(input_schema),
                output: schema(output_schema),
                context: Some(schema("sys/PureContext@1")),
            }),
        },
    }
}

/// Load a real workflow WASM from `target/wasm32-unknown-unknown/<profile>/<file>` and register
/// it in the store, returning a fully populated DefModule.
///
/// This is useful for integration tests that want to exercise actual workflows instead of stubs.
pub fn workflow_module_from_target(
    store: &Arc<TestStore>,
    name: &str,
    wasm_file: &str,
    key_schema: Option<&str>,
    state_schema: &str,
    event_schema: &str,
) -> DefModule {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("../../target/wasm32-unknown-unknown/debug")
        .join(wasm_file);

    if !path.exists() {
        panic!(
            "missing {} — build it first with `cargo build -p aos-sys --target wasm32-unknown-unknown`",
            path.display()
        );
    }

    let bytes = fs::read(&path).expect("read wasm");
    let wasm_hash = Hash::of_bytes(&bytes);
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");
    store.put_blob(&bytes).expect("store wasm blob");

    DefModule {
        name: name.to_string(),
        module_kind: ModuleKind::Workflow,
        wasm_hash: wasm_hash_ref,
        key_schema: key_schema.map(schema),
        abi: ModuleAbi {
            workflow: Some(aos_air_types::WorkflowAbi {
                state: schema(state_schema),
                event: schema(event_schema),
                context: Some(schema("sys/WorkflowContext@1")),
                annotations: None,
                effects_emitted: vec![],
                cap_slots: IndexMap::new(),
            }),
            pure: None,
        },
    }
}

/// Load a real pure WASM module from `target/wasm32-unknown-unknown/<profile>/<file>` and register
/// it in the store, returning a fully populated DefModule.
pub fn pure_module_from_target(
    store: &Arc<TestStore>,
    name: &str,
    wasm_file: &str,
    input_schema: &str,
    output_schema: &str,
) -> DefModule {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .join("../../target/wasm32-unknown-unknown/debug")
        .join(wasm_file);

    if !path.exists() {
        panic!(
            "missing {} — build it first with `cargo build -p aos-sys --target wasm32-unknown-unknown`",
            path.display()
        );
    }

    let bytes = fs::read(&path).expect("read wasm");
    let wasm_hash = Hash::of_bytes(&bytes);
    let wasm_hash_ref = HashRef::new(wasm_hash.to_hex()).expect("hash ref");
    store.put_blob(&bytes).expect("store wasm blob");

    DefModule {
        name: name.to_string(),
        module_kind: ModuleKind::Pure,
        wasm_hash: wasm_hash_ref,
        key_schema: None,
        abi: ModuleAbi {
            workflow: None,
            pure: Some(aos_air_types::PureAbi {
                input: schema(input_schema),
                output: schema(output_schema),
                context: Some(schema("sys/PureContext@1")),
            }),
        },
    }
}

/// Convenience: build a workflow module that emits the supplied domain events (and no state).
pub fn stub_event_emitting_workflow(
    store: &Arc<TestStore>,
    name: impl Into<String>,
    events: Vec<DomainEvent>,
) -> DefModule {
    let output = WorkflowOutput {
        state: None,
        domain_events: events,
        effects: vec![],
        ann: None,
    };
    stub_workflow_module(store, name, &output)
}

/// Helper for synthesizing domain events by name and an already-materialized value.
pub fn domain_event(schema: &str, value: &ExprValue) -> DomainEvent {
    let payload = serde_cbor::to_vec(&expr_value_to_cbor(value)).expect("encode domain event");
    DomainEvent::new(schema.to_string(), payload)
}

fn expr_value_key_to_cbor(key: &ExprValueKey) -> serde_cbor::Value {
    match key {
        ExprValueKey::Int(v) => serde_cbor::Value::Integer((*v).into()),
        ExprValueKey::Nat(v) => serde_cbor::Value::Integer((*v).into()),
        ExprValueKey::Text(v) => serde_cbor::Value::Text(v.clone()),
        ExprValueKey::Hash(v) => serde_cbor::Value::Text(v.clone()),
        ExprValueKey::Uuid(v) => serde_cbor::Value::Text(v.clone()),
    }
}

fn expr_value_to_cbor(value: &ExprValue) -> serde_cbor::Value {
    use serde_cbor::Value as CborValue;
    match value {
        ExprValue::Unit | ExprValue::Null => CborValue::Null,
        ExprValue::Bool(v) => CborValue::Bool(*v),
        ExprValue::Int(v) => CborValue::Integer((*v).into()),
        ExprValue::Nat(v) => CborValue::Integer((*v).into()),
        ExprValue::Dec128(v) => CborValue::Text(v.clone()),
        ExprValue::Bytes(v) => CborValue::Bytes(v.clone()),
        ExprValue::Text(v) => CborValue::Text(v.clone()),
        ExprValue::TimeNs(v) => CborValue::Integer((*v).into()),
        ExprValue::DurationNs(v) => CborValue::Integer((*v).into()),
        ExprValue::Hash(v) => CborValue::Text(v.to_string()),
        ExprValue::Uuid(v) => CborValue::Text(v.clone()),
        ExprValue::List(v) => CborValue::Array(v.iter().map(expr_value_to_cbor).collect()),
        ExprValue::Set(v) => CborValue::Array(v.iter().map(expr_value_key_to_cbor).collect()),
        ExprValue::Map(v) => CborValue::Map(
            v.iter()
                .map(|(k, v)| (expr_value_key_to_cbor(k), expr_value_to_cbor(v)))
                .collect::<BTreeMap<_, _>>(),
        ),
        ExprValue::Record(v) => CborValue::Map(
            v.iter()
                .map(|(k, v)| (CborValue::Text(k.clone()), expr_value_to_cbor(v)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// Utility for building a routing rule from an event schema to a workflow.
pub fn routing_event(schema_name: &str, workflow: &str) -> RoutingEvent {
    RoutingEvent {
        event: schema(schema_name),
        module: workflow.to_string(),
        key_field: None,
    }
}

/// Suggest routing entries for workflow-emitted micro-effect receipts.
///
/// This does not mutate a manifest; it just returns the recommended routes so tests can opt in.
pub fn recommended_receipt_routes<'a>(
    modules: impl IntoIterator<Item = &'a DefModule>,
) -> Vec<RoutingEvent> {
    let catalog = EffectCatalog::from_defs(
        aos_air_types::builtins::builtin_effects()
            .iter()
            .map(|e| e.effect.clone()),
    );
    let mut routes = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for module in modules {
        let Some(workflow) = module.abi.workflow.as_ref() else {
            continue;
        };
        for effect in &workflow.effects_emitted {
            let Some(entry) = catalog.get(effect) else {
                continue;
            };
            if entry.origin_scope != OriginScope::Workflow {
                continue;
            }
            let schema_name = entry.receipt_schema.as_str();
            let key = (schema_name.to_string(), module.name.clone());
            if seen.insert(key) {
                routes.push(routing_event(schema_name, module.name.as_str()));
            }
        }
    }

    routes
}

/// Decodes an effect intent's parameter payload as UTF-8 text, panicking if the payload is not a
/// text literal. Helpful for keeping test assertions concise.
pub fn effect_params_text(intent: &aos_effects::EffectIntent) -> String {
    // Prefer url field from canonical http params if present.
    if let Ok(serde_cbor::Value::Map(map)) =
        serde_cbor::from_slice::<serde_cbor::Value>(&intent.params_cbor)
    {
        if let Some(serde_cbor::Value::Text(url)) = map.get(&serde_cbor::Value::Text("url".into()))
        {
            return url.clone();
        }
    }
    match serde_cbor::from_slice::<ExprValue>(&intent.params_cbor).expect("decode effect params") {
        ExprValue::Text(text) => text,
        other => panic!("expected text params or http url, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// TestWorld: Low-level kernel wrapper for synchronous testing
// ---------------------------------------------------------------------------

use aos_effects::EffectIntent;
use aos_kernel::{Kernel, error::KernelError, journal::Journal, journal::mem::MemJournal};
use serde::Serialize;

/// Wrapper around `Kernel<MemStore>` plus the underlying store for low-level integration tests.
///
/// `TestWorld` provides direct, synchronous access to the kernel for tests that need
/// fine-grained control over kernel operations like `tick_n`, `handle_receipt`, etc.
///
/// For high-level testing through the WorldHost abstraction (with adapters and effect dispatch),
/// use `crate::testhost::TestHost` instead.
pub struct TestWorld {
    pub store: Arc<TestStore>,
    pub kernel: Kernel<TestStore>,
}

impl TestWorld {
    /// Construct a test world with a fresh in-memory store.
    pub fn new(loaded: LoadedManifest) -> Result<Self, KernelError> {
        Self::with_store(new_mem_store(), loaded)
    }

    /// Construct a test world using the provided store (helpful when multiple worlds share blobs).
    pub fn with_store(store: Arc<TestStore>, loaded: LoadedManifest) -> Result<Self, KernelError> {
        let kernel =
            Kernel::from_loaded_manifest(store.clone(), loaded, Box::new(MemJournal::new()))?;
        Ok(Self { store, kernel })
    }

    pub fn with_store_and_journal(
        store: Arc<TestStore>,
        loaded: LoadedManifest,
        journal: Box<dyn Journal>,
    ) -> Result<Self, KernelError> {
        let kernel = Kernel::from_loaded_manifest(store.clone(), loaded, journal)?;
        Ok(Self { store, kernel })
    }

    /// Submit an event encoded as `ExprValue` under the given schema, normalized to the schema.
    pub fn submit_event_value(&mut self, schema: &str, value: &ExprValue) {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel
            .submit_domain_event_result(schema.to_string(), bytes)
            .expect("submit event");
    }

    /// Submit an event and surface normalization/validation errors.
    pub fn submit_event_value_result(
        &mut self,
        schema: &str,
        value: &ExprValue,
    ) -> Result<(), KernelError> {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel
            .submit_domain_event_result(schema.to_string(), bytes)
    }

    /// Submit any serializable payload as an event using the schema string, normalized to the schema.
    pub fn submit_event<T>(&mut self, schema: &str, value: &T)
    where
        T: Serialize,
    {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel
            .submit_domain_event_result(schema.to_string(), bytes)
            .expect("submit event");
    }

    /// Submit any serializable payload as an event, returning the kernel result.
    pub fn submit_event_result<T>(&mut self, schema: &str, value: &T) -> Result<(), KernelError>
    where
        T: Serialize,
    {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel
            .submit_domain_event_result(schema.to_string(), bytes)
    }

    pub fn tick_n(&mut self, n: usize) -> Result<(), KernelError> {
        for _ in 0..n {
            self.kernel.tick()?;
        }
        Ok(())
    }

    /// Convenience passthrough to drain the kernel's effect outbox.
    pub fn drain_effects(&mut self) -> Result<Vec<EffectIntent>, KernelError> {
        self.kernel.drain_effects()
    }
}
