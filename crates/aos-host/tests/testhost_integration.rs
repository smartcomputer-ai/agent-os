//! Integration tests for TestHost using the fixtures module.
//!
//! These tests demonstrate the TestHost API with programmatically built manifests,
//! mirroring the pattern used by examples/00-counter and similar.

use std::collections::HashMap;
use std::sync::Arc;

use aos_air_types::{
    DefModule, DefSchema, Manifest, ModuleAbi, NamedRef, ReducerAbi, Routing, RoutingEvent,
    SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveNat, TypePrimitiveUnit, TypeRecord, TypeRef,
    TypeVariant, catalog::EffectCatalog,
};
use aos_effects::{EffectReceipt, ReceiptStatus};
use aos_host::fixtures::{self, TestStore};
use aos_host::testhost::TestHost;
use aos_kernel::LoadedManifest;
use aos_wasm_abi::{ReducerEffect, ReducerOutput};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// Counter state machine types (mirroring examples/00-counter)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct CounterState {
    pc: CounterPc,
    remaining: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum CounterPc {
    Idle,
    Counting,
    Done,
}

impl Default for CounterPc {
    fn default() -> Self {
        CounterPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent {
    Start { target: u64 },
    Tick,
}

const REDUCER_NAME: &str = "demo/CounterSM@1";
const STATE_SCHEMA: &str = "demo/CounterState@1";
const EVENT_SCHEMA: &str = "demo/CounterEvent@1";
const PC_SCHEMA: &str = "demo/CounterPc@1";

/// Build a counter-like manifest using a stub reducer that returns a predetermined state.
fn build_counter_manifest(store: &Arc<TestStore>, final_state: &CounterState) -> LoadedManifest {
    // Stub reducer that outputs the given final state
    let output = ReducerOutput {
        state: Some(serde_cbor::to_vec(final_state).unwrap()),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };

    let module = fixtures::stub_reducer_module(store, REDUCER_NAME, &output);
    let module_with_abi = DefModule {
        name: module.name.clone(),
        module_kind: module.module_kind.clone(),
        wasm_hash: module.wasm_hash.clone(),
        key_schema: None,
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: SchemaRef::new(STATE_SCHEMA).unwrap(),
                event: SchemaRef::new(EVENT_SCHEMA).unwrap(),
                annotations: None,
                effects_emitted: Vec::new(),
                cap_slots: IndexMap::new(),
            }),
        },
    };

    let pc_schema = counter_pc_schema();
    let state_schema = counter_state_schema();
    let event_schema = counter_event_schema();

    let schemas = HashMap::from([
        (pc_schema.name.clone(), pc_schema.clone()),
        (state_schema.name.clone(), state_schema.clone()),
        (event_schema.name.clone(), event_schema.clone()),
    ]);
    let modules = HashMap::from([(module_with_abi.name.clone(), module_with_abi.clone())]);

    let manifest = Manifest {
        air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
        schemas: vec![
            NamedRef {
                name: PC_SCHEMA.into(),
                hash: fixtures::zero_hash(),
            },
            NamedRef {
                name: STATE_SCHEMA.into(),
                hash: fixtures::zero_hash(),
            },
            NamedRef {
                name: EVENT_SCHEMA.into(),
                hash: fixtures::zero_hash(),
            },
        ],
        modules: vec![NamedRef {
            name: REDUCER_NAME.into(),
            hash: module_with_abi.wasm_hash.clone(),
        }],
        plans: Vec::new(),
        effects: aos_air_types::builtins::builtin_effects()
            .iter()
            .map(|e| NamedRef {
                name: e.effect.name.clone(),
                hash: e.hash_ref.clone(),
            })
            .collect(),
        caps: Vec::new(),
        policies: Vec::new(),
        secrets: Vec::new(),
        defaults: None,
        module_bindings: IndexMap::new(),
        routing: Some(Routing {
            events: vec![RoutingEvent {
                event: SchemaRef::new(EVENT_SCHEMA).unwrap(),
                reducer: REDUCER_NAME.into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
        triggers: Vec::new(),
    };

    let builtin_effects: HashMap<_, _> = aos_air_types::builtins::builtin_effects()
        .iter()
        .map(|e| (e.effect.name.clone(), e.effect.clone()))
        .collect();
    let effect_catalog = EffectCatalog::from_defs(builtin_effects.values().cloned());

    LoadedManifest {
        manifest,
        secrets: Vec::new(),
        modules,
        plans: HashMap::new(),
        effects: builtin_effects,
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
        effect_catalog,
    }
}

fn counter_pc_schema() -> DefSchema {
    let mut variants = IndexMap::new();
    variants.insert("Idle".into(), unit_type());
    variants.insert("Counting".into(), unit_type());
    variants.insert("Done".into(), unit_type());
    DefSchema {
        name: PC_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant: variants }),
    }
}

fn counter_state_schema() -> DefSchema {
    let mut fields = IndexMap::new();
    fields.insert(
        "pc".into(),
        TypeExpr::Ref(TypeRef {
            reference: SchemaRef::new(PC_SCHEMA).unwrap(),
        }),
    );
    fields.insert("remaining".into(), nat_type());
    DefSchema {
        name: STATE_SCHEMA.into(),
        ty: TypeExpr::Record(TypeRecord { record: fields }),
    }
}

fn counter_event_schema() -> DefSchema {
    let mut variants = IndexMap::new();
    let mut start_record = IndexMap::new();
    start_record.insert("target".into(), nat_type());
    variants.insert(
        "Start".into(),
        TypeExpr::Record(TypeRecord {
            record: start_record,
        }),
    );
    variants.insert("Tick".into(), unit_type());
    DefSchema {
        name: EVENT_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant: variants }),
    }
}

fn nat_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Nat(TypePrimitiveNat {
        nat: Default::default(),
    }))
}

fn unit_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Unit(TypePrimitiveUnit {
        unit: Default::default(),
    }))
}

#[tokio::test]
async fn testhost_from_loaded_manifest_counter_flow() {
    // This test mirrors the counter example flow but uses TestHost
    let store = fixtures::new_mem_store();

    // Expected final state after counting down from 3
    let final_state = CounterState {
        pc: CounterPc::Done,
        remaining: 0,
    };

    let loaded = build_counter_manifest(&store, &final_state);
    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Send Start event
    let start_event = CounterEvent::Start { target: 3 };
    let start_cbor = serde_cbor::to_vec(&start_event).unwrap();
    host.send_event_cbor(EVENT_SCHEMA, start_cbor).unwrap();

    // Run cycle
    let cycle = host.run_cycle_batch().await.unwrap();
    assert!(cycle.initial_drain.idle);

    // Check state
    let state: CounterState = host.state(REDUCER_NAME).unwrap();
    assert_eq!(state.pc, CounterPc::Done);
    assert_eq!(state.remaining, 0);

    // Snapshot
    host.snapshot().unwrap();
}

#[tokio::test]
async fn testhost_drain_effects_empty_for_counter() {
    let store = fixtures::new_mem_store();
    let final_state = CounterState {
        pc: CounterPc::Done,
        remaining: 0,
    };
    let loaded = build_counter_manifest(&store, &final_state);
    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Counter reducer doesn't emit effects
    let effects = host.drain_effects();
    assert!(effects.is_empty());
}

#[tokio::test]
async fn testhost_kernel_escape_hatch() {
    let store = fixtures::new_mem_store();
    let final_state = CounterState {
        pc: CounterPc::Idle,
        remaining: 0,
    };
    let loaded = build_counter_manifest(&store, &final_state);
    let host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Access kernel directly via escape hatch
    let heights = host.kernel().heights();
    assert_eq!(heights.head, 0);
}

#[tokio::test]
async fn testhost_state_bytes_and_typed_state_match() {
    let store = fixtures::new_mem_store();
    let expected_state = CounterState {
        pc: CounterPc::Counting,
        remaining: 5,
    };
    let loaded = build_counter_manifest(&store, &expected_state);
    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Send event to initialize state
    let start_event = CounterEvent::Start { target: 5 };
    let start_cbor = serde_cbor::to_vec(&start_event).unwrap();
    host.send_event_cbor(EVENT_SCHEMA, start_cbor).unwrap();
    host.run_cycle_batch().await.unwrap();

    // Compare both access methods
    let bytes = host.state_bytes(REDUCER_NAME).unwrap();
    let typed: CounterState = host.state(REDUCER_NAME).unwrap();
    let from_bytes: CounterState = serde_cbor::from_slice(bytes).unwrap();

    assert_eq!(typed, from_bytes);
    assert_eq!(typed, expected_state);
}

/// Test using the fixtures module helper to build a minimal manifest
#[tokio::test]
async fn testhost_with_fixtures_build_loaded_manifest() {
    let store = fixtures::new_mem_store();

    // Use fixtures helpers to build a simple reducer stub
    let output = ReducerOutput {
        state: Some(vec![0xAA, 0xBB]),
        domain_events: vec![],
        effects: vec![],
        ann: None,
    };
    let module = fixtures::stub_reducer_module(&store, "test/Reducer@1", &output);

    // Build manifest using fixtures helper
    let loaded = fixtures::build_loaded_manifest(
        vec![],                                           // no plans
        vec![],                                           // no triggers
        vec![module],                                     // one reducer
        vec![fixtures::routing_event("test/Event@1", "test/Reducer@1")],
    );

    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Send event to trigger reducer
    host.send_event("test/Event@1", serde_json::json!({}))
        .unwrap();
    host.run_cycle_batch().await.unwrap();

    // Check state was set
    let bytes = host.state_bytes("test/Reducer@1").unwrap();
    assert_eq!(bytes, &vec![0xAA, 0xBB]);
}

/// Replay smoke test: open → cycle → snapshot → check heights
#[tokio::test]
async fn testhost_replay_smoke() {
    let store = fixtures::new_mem_store();
    let final_state = CounterState {
        pc: CounterPc::Done,
        remaining: 0,
    };
    let loaded = build_counter_manifest(&store, &final_state);
    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Initial heights
    let initial_heights = host.heights();
    assert_eq!(initial_heights.head, 0);

    // Send event and run
    let start_event = CounterEvent::Start { target: 3 };
    let start_cbor = serde_cbor::to_vec(&start_event).unwrap();
    host.send_event_cbor(EVENT_SCHEMA, start_cbor).unwrap();
    host.run_cycle_batch().await.unwrap();

    // Heights should advance
    let after_heights = host.heights();
    assert!(after_heights.head > initial_heights.head);

    // Snapshot
    host.snapshot().unwrap();

    // State should still be accessible
    let state: CounterState = host.state(REDUCER_NAME).unwrap();
    assert_eq!(state.pc, CounterPc::Done);
}

// Timer test types
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetParams {
    deliver_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

/// Test timer micro-effect flow: reducer emits timer.set, we drain and inject receipt
#[tokio::test]
async fn testhost_timer_effect_flow() {
    let store = fixtures::new_mem_store();

    // Build reducer that emits a timer.set effect
    let timer_params = TimerSetParams {
        deliver_at_ns: 1_000_000,
        key: Some("test-timer".into()),
    };
    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&timer_params).unwrap(),
        "default",
    );

    let output = ReducerOutput {
        state: Some(vec![0x01]), // "awaiting" state
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };

    let module = fixtures::stub_reducer_module(&store, "test/TimerReducer@1", &output);

    // Build manifest with routing
    let loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event("test/TimerEvent@1", "test/TimerReducer@1")],
    );

    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Send event to trigger reducer
    host.send_event("test/TimerEvent@1", serde_json::json!({}))
        .unwrap();

    // Run cycle - this will drain reducer but not dispatch effects yet
    host.kernel_mut().tick_until_idle().unwrap();

    // Drain the timer effect
    let effects = host.drain_effects();
    assert_eq!(effects.len(), 1);
    let timer_intent = &effects[0];
    assert_eq!(timer_intent.kind.as_str(), "timer.set");

    // Parse and verify params
    let parsed_params: TimerSetParams =
        serde_cbor::from_slice(&timer_intent.params_cbor).unwrap();
    assert_eq!(parsed_params.deliver_at_ns, 1_000_000);
    assert_eq!(parsed_params.key, Some("test-timer".into()));

    // Inject synthetic receipt (simulating timer fire)
    let receipt_payload = TimerSetReceipt {
        delivered_at_ns: 1_000_000,
        key: Some("test-timer".into()),
    };
    let receipt = EffectReceipt {
        intent_hash: timer_intent.intent_hash,
        adapter_id: "adapter.timer.test".into(),
        status: ReceiptStatus::Ok,
        payload_cbor: serde_cbor::to_vec(&receipt_payload).unwrap(),
        cost_cents: Some(0),
        signature: vec![],
    };

    host.inject_receipt(receipt).unwrap();
    host.kernel_mut().tick_until_idle().unwrap();

    // State should still be accessible after receipt processing
    let state_bytes = host.state_bytes("test/TimerReducer@1").unwrap();
    assert_eq!(state_bytes, &vec![0x01]);
}

/// Test that run_cycle_batch handles effects via stub adapters
#[tokio::test]
async fn testhost_run_cycle_batch_with_timer_effect() {
    let store = fixtures::new_mem_store();

    // Build reducer that emits a timer.set effect
    let timer_params = TimerSetParams {
        deliver_at_ns: 500_000,
        key: None,
    };
    let effect = ReducerEffect::with_cap_slot(
        aos_effects::EffectKind::TIMER_SET,
        serde_cbor::to_vec(&timer_params).unwrap(),
        "default",
    );

    let output = ReducerOutput {
        state: Some(vec![0xBB]),
        domain_events: vec![],
        effects: vec![effect],
        ann: None,
    };

    let module = fixtures::stub_reducer_module(&store, "test/TimerReducer@1", &output);
    let loaded = fixtures::build_loaded_manifest(
        vec![],
        vec![],
        vec![module],
        vec![fixtures::routing_event("test/TimerEvent@1", "test/TimerReducer@1")],
    );

    let mut host = TestHost::from_loaded_manifest(store, loaded).unwrap();

    // Send event
    host.send_event("test/TimerEvent@1", serde_json::json!({}))
        .unwrap();

    // run_cycle_batch should dispatch effects via stub adapters
    let cycle = host.run_cycle_batch().await.unwrap();

    // Should have dispatched 1 effect and applied 1 receipt (from stub)
    assert_eq!(cycle.effects_dispatched, 1);
    assert_eq!(cycle.receipts_applied, 1);

    // State should be set
    let state_bytes = host.state_bytes("test/TimerReducer@1").unwrap();
    assert_eq!(state_bytes, &vec![0xBB]);
}
