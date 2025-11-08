//! Test utilities for exercising the AgentOS kernel with deterministic fixtures.

use std::collections::HashMap;
use std::sync::Arc;

use aos_air_exec::Value as ExprValue;
use aos_air_types::{
    CapGrant, CapType, DefCap, DefModule, DefPlan, Expr, ExprConst, ExprRef, HashRef, Manifest,
    ManifestDefaults, ModuleAbi, ModuleBinding, ModuleKind, Name, NamedRef, Routing, RoutingEvent,
    SchemaRef, Trigger, TypeExpr, TypeRecord, ValueLiteral, ValueRecord,
};
use aos_effects::EffectIntent;
use aos_kernel::{Kernel, error::KernelError, manifest::LoadedManifest};
use aos_store::{MemStore, Store};
use aos_wasm_abi::{DomainEvent, ReducerOutput};
use indexmap::IndexMap;
use serde::Serialize;
use serde_cbor;
use wat::parse_str;

/// In-memory store alias used across fixtures.
pub type TestStore = MemStore;

/// Shared helpers for constructing manifests, expressions, and reusable reducer/plan stubs.
pub mod fixtures {
    use super::*;

    pub const START_SCHEMA: &str = "com.acme/Start@1";
    pub const SYS_TIMER_FIRED: &str = "sys/TimerFired@1";

    /// Returns a schema reference for reuse in manifests and plans.
    pub fn schema(name: &str) -> SchemaRef {
        SchemaRef::new(name).unwrap()
    }

    /// Builds a plan expression that yields a text literal.
    pub fn text_expr(value: &str) -> Expr {
        Expr::Const(ExprConst::Text {
            text: value.to_string(),
        })
    }

    /// Builds a plan expression that yields a boolean literal.
    pub fn bool_expr(value: bool) -> Expr {
        Expr::Const(ExprConst::Bool { bool: value })
    }

    /// References a previously bound plan variable (e.g., `@var:req`).
    pub fn var_expr(name: &str) -> Expr {
        Expr::Ref(ExprRef {
            reference: format!("@var:{name}"),
        })
    }

    /// References a field on the plan input (e.g., `@plan.input.order_id`).
    pub fn plan_input_expr(field: &str) -> Expr {
        Expr::Ref(ExprRef {
            reference: format!("@plan.input.{field}"),
        })
    }

    /// Convenience helper for synthesizing a record literal for plan inputs/events.
    pub fn plan_input_record(fields: Vec<(&str, ExprValue)>) -> ExprValue {
        ExprValue::Record(IndexMap::from_iter(
            fields.into_iter().map(|(k, v)| (k.to_string(), v)),
        ))
    }

    /// Trigger helper that wires the standard `START_SCHEMA` to the provided plan.
    pub fn start_trigger(plan: &str) -> Trigger {
        Trigger {
            event: schema(START_SCHEMA),
            plan: plan.to_string(),
            correlate_by: None,
        }
    }

    /// Trigger helper for the built-in timer receipt schema.
    pub fn timer_trigger(plan: &str) -> Trigger {
        Trigger {
            event: schema(SYS_TIMER_FIRED),
            plan: plan.to_string(),
            correlate_by: None,
        }
    }

    /// Returns the zero hash helper used as a placeholder for plan references.
    pub fn zero_hash() -> HashRef {
        HashRef::new("sha256:0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap()
    }

    /// Builds a `LoadedManifest` from already-parsed plan and module definitions.
    pub fn build_loaded_manifest(
        mut plans: Vec<DefPlan>,
        triggers: Vec<Trigger>,
        mut modules: Vec<DefModule>,
        routing_events: Vec<RoutingEvent>,
    ) -> LoadedManifest {
        let plan_refs: Vec<NamedRef> = plans
            .iter()
            .map(|plan| NamedRef {
                name: plan.name.clone(),
                hash: zero_hash(),
            })
            .collect();
        let module_refs: Vec<NamedRef> = modules
            .iter()
            .map(|module| NamedRef {
                name: module.name.clone(),
                hash: module.wasm_hash.clone(),
            })
            .collect();

        let routing = if routing_events.is_empty() {
            None
        } else {
            Some(Routing {
                events: routing_events,
                inboxes: vec![],
            })
        };

        let mut manifest = Manifest {
            schemas: vec![],
            modules: module_refs,
            plans: plan_refs,
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: Default::default(),
            routing,
            triggers,
        };

        let modules_map: HashMap<Name, DefModule> = modules
            .drain(..)
            .map(|module| (module.name.clone(), module))
            .collect();
        let plans_map: HashMap<Name, DefPlan> = plans
            .drain(..)
            .map(|plan| (plan.name.clone(), plan))
            .collect();

        let caps = attach_test_capabilities(&mut manifest, modules_map.keys());

        LoadedManifest {
            manifest,
            modules: modules_map,
            plans: plans_map,
            caps,
            policies: HashMap::new(),
        }
    }

    /// Populates the manifest with default capability grants and module slot bindings so reducers
    /// can emit timer/blob effects without extra ceremony.
    pub fn attach_test_capabilities<'a, I>(
        manifest: &mut Manifest,
        modules: I,
    ) -> HashMap<Name, DefCap>
    where
        I: IntoIterator<Item = &'a Name>,
    {
        manifest.defaults = Some(ManifestDefaults {
            policy: None,
            cap_grants: vec![cap_http_grant(), timer_cap_grant(), blob_cap_grant()],
        });
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
        ])
    }

    pub fn cap_http_grant() -> CapGrant {
        CapGrant {
            name: "cap_http".into(),
            cap: "sys/http.out@1".into(),
            params: empty_value_literal(),
            expiry_ns: None,
            budget: None,
        }
    }

    pub fn timer_cap_grant() -> CapGrant {
        CapGrant {
            name: "timer_cap".into(),
            cap: "sys/timer@1".into(),
            params: empty_value_literal(),
            expiry_ns: None,
            budget: None,
        }
    }

    pub fn blob_cap_grant() -> CapGrant {
        CapGrant {
            name: "blob_cap".into(),
            cap: "sys/blob@1".into(),
            params: empty_value_literal(),
            expiry_ns: None,
            budget: None,
        }
    }

    /// Minimal HTTP capability definition used inside tests.
    pub fn http_defcap() -> DefCap {
        DefCap {
            name: "sys/http.out@1".into(),
            cap_type: CapType::HttpOut,
            schema: TypeExpr::Record(TypeRecord {
                record: IndexMap::new(),
            }),
        }
    }

    /// Minimal Timer capability definition used inside tests.
    pub fn timer_defcap() -> DefCap {
        DefCap {
            name: "sys/timer@1".into(),
            cap_type: CapType::Timer,
            schema: TypeExpr::Record(TypeRecord {
                record: IndexMap::new(),
            }),
        }
    }

    pub fn blob_defcap() -> DefCap {
        DefCap {
            name: "sys/blob@1".into(),
            cap_type: CapType::Blob,
            schema: TypeExpr::Record(TypeRecord {
                record: IndexMap::new(),
            }),
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
    /// `ReducerOutput` bytes. Useful for reducers that simply emit domain events or effects.
    pub fn stub_reducer_module(
        store: &Arc<TestStore>,
        name: impl Into<String>,
        output: &ReducerOutput,
    ) -> DefModule {
        let output_bytes = output.encode().expect("encode reducer output");
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
            module_kind: ModuleKind::Reducer,
            wasm_hash: wasm_hash_ref,
            key_schema: None,
            abi: ModuleAbi { reducer: None },
        }
    }

    /// Convenience: build a reducer module that emits the supplied domain events (and no state).
    pub fn stub_event_emitting_reducer(
        store: &Arc<TestStore>,
        name: impl Into<String>,
        events: Vec<DomainEvent>,
    ) -> DefModule {
        let output = ReducerOutput {
            state: None,
            domain_events: events,
            effects: vec![],
            ann: None,
        };
        stub_reducer_module(store, name, &output)
    }

    /// Helper for synthesizing domain events by name and an already-materialized value.
    pub fn domain_event(schema: &str, value: &ExprValue) -> DomainEvent {
        DomainEvent::new(
            schema.to_string(),
            serde_cbor::to_vec(value).expect("encode domain event"),
        )
    }

    /// Utility for building a routing rule from an event schema to a reducer.
    pub fn routing_event(schema_name: &str, reducer: &str) -> RoutingEvent {
        RoutingEvent {
            event: schema(schema_name),
            reducer: reducer.to_string(),
            key_field: None,
        }
    }
}

/// Wrapper around `Kernel<MemStore>` plus the underlying store for integration tests.
pub struct TestWorld {
    pub store: Arc<TestStore>,
    pub kernel: Kernel<TestStore>,
}

/// Decodes an effect intent's parameter payload as UTF-8 text, panicking if the payload is not a
/// text literal. Helpful for keeping test assertions concise.
pub fn effect_params_text(intent: &EffectIntent) -> String {
    match serde_cbor::from_slice::<ExprValue>(&intent.params_cbor).expect("decode effect params") {
        ExprValue::Text(text) => text,
        other => panic!("expected text params, got {:?}", other),
    }
}

impl TestWorld {
    /// Construct a test world with a fresh in-memory store.
    pub fn new(loaded: LoadedManifest) -> Result<Self, KernelError> {
        Self::with_store(fixtures::new_mem_store(), loaded)
    }

    /// Construct a test world using the provided store (helpful when multiple worlds share blobs).
    pub fn with_store(store: Arc<TestStore>, loaded: LoadedManifest) -> Result<Self, KernelError> {
        let kernel = Kernel::from_loaded_manifest(store.clone(), loaded)?;
        Ok(Self { store, kernel })
    }

    /// Submit an event encoded as `ExprValue` under the given schema.
    pub fn submit_event_value(&mut self, schema: &str, value: &ExprValue) {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel.submit_domain_event(schema.to_string(), bytes);
    }

    /// Submit any serializable payload as an event using the schema string.
    pub fn submit_event<T>(&mut self, schema: &str, value: &T)
    where
        T: Serialize,
    {
        let bytes = serde_cbor::to_vec(value).expect("encode event");
        self.kernel.submit_domain_event(schema.to_string(), bytes);
    }

    pub fn tick_n(&mut self, n: usize) -> Result<(), KernelError> {
        for _ in 0..n {
            self.kernel.tick()?;
        }
        Ok(())
    }

    /// Convenience passthrough to drain the kernel's effect outbox.
    pub fn drain_effects(&mut self) -> Vec<EffectIntent> {
        self.kernel.drain_effects()
    }
}
