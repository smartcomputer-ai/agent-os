//! Counter demo with a fully code-wired manifest.
//!
//! Unlike the later examples, `examples/00-counter` does not yet ship AIR JSON assets.
//! We intentionally build the manifest in Rust here so users can see how to handcraft
//! schemas/modules for tiny reducers before graduating to the asset pipeline.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::{
    DefModule, DefSchema, HashRef, Manifest, ModuleAbi, ModuleKind, NamedRef, ReducerAbi, Routing,
    RoutingEvent, SchemaRef, TypeExpr, TypePrimitive, TypePrimitiveNat, TypePrimitiveUnit,
    TypeRecord, TypeRef, TypeVariant,
};
use aos_cbor::Hash;
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::support::util;

const REDUCER_NAME: &str = "demo/CounterSM@1";
const STATE_SCHEMA: &str = "demo/CounterState@1";
const EVENT_SCHEMA: &str = "demo/CounterEvent@1";
const PC_SCHEMA: &str = "demo/CounterPc@1";
const TARGET_COUNT: u64 = 3;

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

pub fn run(example_root: &Path) -> Result<()> {
    util::reset_journal(example_root)?;
    let wasm_bytes = util::compile_reducer("examples/00-counter/reducer")?;
    let store = Arc::new(FsStore::open(example_root).context("open FsStore")?);
    let loaded = build_loaded_manifest(store.clone(), &wasm_bytes).context("build manifest")?;
    let journal = Box::new(FsJournal::open(example_root)?);
    let kernel_config = util::kernel_config(example_root)?;
    let mut kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        journal,
        kernel_config.clone(),
    )?;

    println!("→ Counter demo (target {TARGET_COUNT})");
    drive_counter(&mut kernel).context("drive counter")?;

    let final_state_bytes = current_state_bytes(&kernel)?;
    let final_state: CounterState = serde_cbor::from_slice(&final_state_bytes)?;
    println!(
        "   final state: pc={:?}, remaining={}",
        final_state.pc, final_state.remaining
    );

    drop(kernel);

    // Replay and compare state bytes.
    let loaded_replay = build_loaded_manifest(store.clone(), &wasm_bytes)?;
    let replay_journal = Box::new(FsJournal::open(example_root)?);
    let mut replay = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded_replay,
        replay_journal,
        kernel_config,
    )?;
    replay.tick_until_idle()?;
    let replay_state = current_state_bytes(&replay)?;
    if replay_state != final_state_bytes {
        return Err(anyhow!("replay mismatch: states diverged"));
    }
    let state_hash = Hash::of_bytes(&final_state_bytes).to_hex();
    println!("   replay check: OK (state hash {state_hash})\n");

    Ok(())
}

fn drive_counter(kernel: &mut Kernel<FsStore>) -> Result<()> {
    println!("     start (target {TARGET_COUNT})");
    submit_event(
        kernel,
        CounterEvent::Start {
            target: TARGET_COUNT,
        },
    )?;
    for tick in 1..=TARGET_COUNT {
        submit_event(kernel, CounterEvent::Tick)?;
        println!("     tick #{tick}");
    }
    Ok(())
}

fn submit_event(kernel: &mut Kernel<FsStore>, event: CounterEvent) -> Result<()> {
    let payload = serde_cbor::to_vec(&event)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn current_state_bytes(kernel: &Kernel<FsStore>) -> Result<Vec<u8>> {
    kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing state for {REDUCER_NAME}"))
}

fn build_loaded_manifest(store: Arc<FsStore>, wasm_bytes: &[u8]) -> Result<LoadedManifest> {
    let pc_schema = counter_pc_schema();
    let state_schema = counter_state_schema();
    let event_schema = counter_event_schema();
    let wasm_hash = store.put_blob(wasm_bytes)?;
    let module = counter_module(wasm_hash)?;

    let pc_hash = store.put_node(&pc_schema)?;
    let state_hash = store.put_node(&state_schema)?;
    let event_hash = store.put_node(&event_schema)?;
    let module_hash = store.put_node(&module)?;

    let schemas = HashMap::from([
        (pc_schema.name.clone(), pc_schema.clone()),
        (state_schema.name.clone(), state_schema.clone()),
        (event_schema.name.clone(), event_schema.clone()),
    ]);
    let modules = HashMap::from([(module.name.clone(), module.clone())]);

    let manifest = Manifest {
        schemas: vec![
            named_ref(PC_SCHEMA, pc_hash)?,
            named_ref(STATE_SCHEMA, state_hash)?,
            named_ref(EVENT_SCHEMA, event_hash)?,
        ],
        modules: vec![named_ref(REDUCER_NAME, module_hash)?],
        plans: Vec::new(),
        caps: Vec::new(),
        policies: Vec::new(),
        defaults: None,
        module_bindings: IndexMap::new(),
        routing: Some(Routing {
            events: vec![RoutingEvent {
                event: schema_ref(EVENT_SCHEMA)?,
                reducer: REDUCER_NAME.into(),
                key_field: None,
            }],
            inboxes: Vec::new(),
        }),
        triggers: Vec::new(),
    };

    Ok(LoadedManifest {
        manifest,
        modules,
        plans: HashMap::new(),
        caps: HashMap::new(),
        policies: HashMap::new(),
        schemas,
    })
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
            reference: schema_ref(PC_SCHEMA).expect("pc schema ref"),
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

fn counter_module(wasm_hash: Hash) -> Result<DefModule> {
    Ok(DefModule {
        name: REDUCER_NAME.into(),
        module_kind: ModuleKind::Reducer,
        wasm_hash: HashRef::new(wasm_hash.to_hex())?,
        key_schema: None,
        abi: ModuleAbi {
            reducer: Some(ReducerAbi {
                state: schema_ref(STATE_SCHEMA)?,
                event: schema_ref(EVENT_SCHEMA)?,
                annotations: None,
                effects_emitted: Vec::new(),
                cap_slots: IndexMap::new(),
            }),
        },
    })
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

fn named_ref(name: &str, hash: Hash) -> Result<NamedRef> {
    Ok(NamedRef {
        name: name.into(),
        hash: HashRef::new(hash.to_hex())?,
    })
}

fn schema_ref(name: &str) -> Result<SchemaRef> {
    SchemaRef::new(name).map_err(|err| anyhow!("schema ref '{name}' invalid: {err}"))
}
