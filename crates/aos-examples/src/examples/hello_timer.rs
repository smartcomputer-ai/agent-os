use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use aos_air_types::{
    CapGrant, CapType, DefCap, DefModule, DefPolicy, DefSchema, EffectKind as AirEffectKind,
    HashRef, Manifest, ManifestDefaults, ModuleAbi, ModuleBinding, ModuleKind, Name, NamedRef,
    PolicyDecision, PolicyMatch, PolicyRule, ReducerAbi, Routing, RoutingEvent, SchemaRef,
    TypeExpr, TypeOption, TypePrimitive, TypePrimitiveNat, TypePrimitiveText, TypePrimitiveUnit,
    TypeRecord, TypeRef, TypeVariant, ValueLiteral, ValueRecord, builtins,
};
use aos_cbor::Hash;
use aos_effects::{EffectKind as EffectsEffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::examples::util;

const REDUCER_NAME: &str = "demo/TimerSM@1";
const STATE_SCHEMA: &str = "demo/TimerState@1";
const EVENT_SCHEMA: &str = "demo/TimerEvent@1";
const PC_SCHEMA: &str = "demo/TimerPc@1";
const TIMER_CAP_NAME: &str = "sys/timer@1";
const TIMER_POLICY_NAME: &str = "demo/default_policy@1";
const TIMER_GRANT: &str = "timer_grant";
const SYS_TIMER_FIRED: &str = "sys/TimerFired@1";
const SYS_TIMER_SET_PARAMS: &str = "sys/TimerSetParams@1";
const SYS_TIMER_SET_RECEIPT: &str = "sys/TimerSetReceipt@1";
const ADAPTER_ID: &str = "adapter.timer.fake";
const START_KEY: &str = "demo-key";
const DELIVER_AT_NS: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct TimerState {
    pc: TimerPc,
    key: Option<String>,
    deadline_ns: Option<u64>,
    fired_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum TimerPc {
    Idle,
    Awaiting,
    Done,
    TimedOut,
}

impl Default for TimerPc {
    fn default() -> Self {
        TimerPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    deliver_at_ns: u64,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetParams {
    deliver_at_ns: u64,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerSetReceipt {
    delivered_at_ns: u64,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimerFiredEvent {
    intent_hash: HashRef,
    reducer: String,
    effect_kind: String,
    adapter_id: String,
    status: String,
    requested: TimerSetParams,
    receipt: TimerSetReceipt,
    cost_cents: u64,
    signature: Vec<u8>,
}

pub fn run(example_root: &Path) -> Result<()> {
    util::reset_journal(example_root)?;
    let wasm_bytes = util::compile_reducer("examples/01-hello-timer/reducer")?;
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

    println!("→ Hello Timer demo");
    drive_timer_demo(&mut kernel).context("drive timer demo")?;

    let final_state = read_state(&kernel)?;
    println!(
        "   final state: pc={:?}, key={:?}, fired_key={:?}",
        final_state.pc, final_state.key, final_state.fired_key
    );

    drop(kernel);

    let replay_loaded = build_loaded_manifest(store.clone(), &wasm_bytes)?;
    let replay_journal = Box::new(FsJournal::open(example_root)?);
    let replay = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        replay_loaded,
        replay_journal,
        kernel_config,
    )?;
    let replay_state = read_state(&replay)?;
    if replay_state != final_state {
        return Err(anyhow!("replay mismatch: states diverged"));
    }
    let state_hash = Hash::of_cbor(&final_state)?.to_hex();
    println!("   replay check: OK (state hash {state_hash})\n");

    Ok(())
}

fn drive_timer_demo(kernel: &mut Kernel<FsStore>) -> Result<()> {
    println!("     start key={START_KEY} deliver_ns={DELIVER_AT_NS}");
    let start = StartEvent {
        deliver_at_ns: DELIVER_AT_NS,
        key: START_KEY.into(),
    };
    submit_start(kernel, &start)?;
    synthesize_timer_receipts(kernel)?;
    Ok(())
}

fn submit_start(kernel: &mut Kernel<FsStore>, start: &StartEvent) -> Result<()> {
    let payload = serde_cbor::to_vec(start)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn synthesize_timer_receipts(kernel: &mut Kernel<FsStore>) -> Result<()> {
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            ensure!(
                intent.kind.as_str() == EffectsEffectKind::TIMER_SET,
                "unexpected effect {:?}",
                intent.kind
            );
            let params: TimerSetParams = serde_cbor::from_slice(&intent.params_cbor)?;
            println!(
                "     timer.set -> key={} deliver_ns={}",
                params.key, params.deliver_at_ns
            );
            let receipt_payload = TimerSetReceipt {
                delivered_at_ns: params.deliver_at_ns,
                key: params.key.clone(),
            };
            let receipt = EffectReceipt {
                intent_hash: intent.intent_hash,
                adapter_id: ADAPTER_ID.into(),
                status: ReceiptStatus::Ok,
                payload_cbor: serde_cbor::to_vec(&receipt_payload)?,
                cost_cents: Some(0),
                signature: vec![0; 64],
            };
            kernel.handle_receipt(receipt)?;
            kernel.tick_until_idle()?;
            println!("     timer fired (synthetic receipt)");
        }
    }
    Ok(())
}

fn read_state(kernel: &Kernel<FsStore>) -> Result<TimerState> {
    let bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing state for {REDUCER_NAME}"))?;
    Ok(serde_cbor::from_slice(&bytes)?)
}

fn build_loaded_manifest(store: Arc<FsStore>, wasm_bytes: &[u8]) -> Result<LoadedManifest> {
    let pc_schema = timer_pc_schema();
    let state_schema = timer_state_schema();
    let event_schema = timer_event_schema()?;
    let cap = timer_cap_schema();
    let policy = timer_policy();
    let module = timer_module(store.put_blob(wasm_bytes)?)?;

    let pc_hash = store.put_node(&pc_schema)?;
    let state_hash = store.put_node(&state_schema)?;
    let event_hash = store.put_node(&event_schema)?;
    let cap_hash = store.put_node(&cap)?;
    let policy_hash = store.put_node(&policy)?;
    let module_hash = store.put_node(&module)?;

    let mut schemas: HashMap<Name, DefSchema> = HashMap::from([
        (pc_schema.name.clone(), pc_schema.clone()),
        (state_schema.name.clone(), state_schema.clone()),
        (event_schema.name.clone(), event_schema.clone()),
    ]);
    for name in [SYS_TIMER_SET_PARAMS, SYS_TIMER_SET_RECEIPT, SYS_TIMER_FIRED] {
        if let Some(builtin) = builtins::find_builtin_schema(name) {
            schemas.insert(name.to_string(), builtin.schema.clone());
        }
    }

    let manifest = Manifest {
        schemas: vec![
            named_ref(PC_SCHEMA, pc_hash)?,
            named_ref(STATE_SCHEMA, state_hash)?,
            named_ref(EVENT_SCHEMA, event_hash)?,
            builtin_named_ref(SYS_TIMER_SET_PARAMS)?,
            builtin_named_ref(SYS_TIMER_SET_RECEIPT)?,
            builtin_named_ref(SYS_TIMER_FIRED)?,
        ],
        modules: vec![named_ref(REDUCER_NAME, module_hash)?],
        plans: Vec::new(),
        caps: vec![named_ref(TIMER_CAP_NAME, cap_hash)?],
        policies: vec![named_ref(TIMER_POLICY_NAME, policy_hash)?],
        defaults: Some(ManifestDefaults {
            policy: Some(TIMER_POLICY_NAME.into()),
            cap_grants: vec![CapGrant {
                name: TIMER_GRANT.into(),
                cap: TIMER_CAP_NAME.into(),
                params: empty_record(),
                expiry_ns: None,
                budget: None,
            }],
        }),
        module_bindings: {
            let mut bindings = IndexMap::new();
            let mut slots = IndexMap::new();
            slots.insert("timer".into(), TIMER_GRANT.into());
            bindings.insert(REDUCER_NAME.into(), ModuleBinding { slots });
            bindings
        },
        routing: Some(Routing {
            events: vec![
                RoutingEvent {
                    event: schema_ref(EVENT_SCHEMA)?,
                    reducer: REDUCER_NAME.into(),
                    key_field: None,
                },
                RoutingEvent {
                    event: schema_ref(SYS_TIMER_FIRED)?,
                    reducer: REDUCER_NAME.into(),
                    key_field: None,
                },
            ],
            inboxes: Vec::new(),
        }),
        triggers: Vec::new(),
    };

    Ok(LoadedManifest {
        manifest,
        modules: HashMap::from([(module.name.clone(), module)]),
        plans: HashMap::new(),
        caps: HashMap::from([(cap.name.clone(), cap)]),
        policies: HashMap::from([(policy.name.clone(), policy)]),
        schemas,
    })
}

fn timer_pc_schema() -> DefSchema {
    let mut variant = IndexMap::new();
    variant.insert("Idle".into(), unit_type());
    variant.insert("Awaiting".into(), unit_type());
    variant.insert("Done".into(), unit_type());
    variant.insert("TimedOut".into(), unit_type());
    DefSchema {
        name: PC_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant }),
    }
}

fn timer_state_schema() -> DefSchema {
    let mut fields = IndexMap::new();
    fields.insert(
        "pc".into(),
        TypeExpr::Ref(TypeRef {
            reference: schema_ref(PC_SCHEMA).expect("pc schema ref"),
        }),
    );
    fields.insert("key".into(), option_text());
    fields.insert("deadline_ns".into(), option_nat());
    fields.insert("fired_key".into(), option_text());
    DefSchema {
        name: STATE_SCHEMA.into(),
        ty: TypeExpr::Record(TypeRecord { record: fields }),
    }
}

fn timer_event_schema() -> Result<DefSchema> {
    let mut variant = IndexMap::new();
    let mut start_fields = IndexMap::new();
    start_fields.insert("deliver_at_ns".into(), nat_type());
    start_fields.insert("key".into(), text_type());
    variant.insert(
        "Start".into(),
        TypeExpr::Record(TypeRecord {
            record: start_fields,
        }),
    );
    variant.insert(
        "Fired".into(),
        TypeExpr::Ref(TypeRef {
            reference: schema_ref(SYS_TIMER_FIRED)?,
        }),
    );
    Ok(DefSchema {
        name: EVENT_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant }),
    })
}

fn timer_module(wasm_hash: Hash) -> Result<DefModule> {
    let mut cap_slots = IndexMap::new();
    cap_slots.insert("timer".into(), CapType::Timer);
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
                effects_emitted: vec![AirEffectKind::TimerSet],
                cap_slots,
            }),
        },
    })
}

fn timer_cap_schema() -> DefCap {
    DefCap {
        name: TIMER_CAP_NAME.into(),
        cap_type: CapType::Timer,
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
    }
}

fn timer_policy() -> DefPolicy {
    DefPolicy {
        name: TIMER_POLICY_NAME.into(),
        rules: vec![PolicyRule {
            when: PolicyMatch {
                effect_kind: Some(AirEffectKind::TimerSet),
                origin_kind: Some(aos_air_types::OriginKind::Reducer),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        }],
    }
}

fn named_ref(name: &str, hash: Hash) -> Result<NamedRef> {
    Ok(NamedRef {
        name: name.into(),
        hash: HashRef::new(hash.to_hex())?,
    })
}

fn builtin_named_ref(name: &str) -> Result<NamedRef> {
    let builtin = builtins::find_builtin_schema(name)
        .ok_or_else(|| anyhow!("builtin schema '{name}' not found"))?;
    Ok(NamedRef {
        name: name.into(),
        hash: builtin.hash_ref.clone(),
    })
}

fn schema_ref(name: &str) -> Result<SchemaRef> {
    SchemaRef::new(name).map_err(|err| anyhow!("schema ref '{name}' invalid: {err}"))
}

fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: Default::default(),
    }))
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

fn option_text() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(text_type()),
    })
}

fn option_nat() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(nat_type()),
    })
}

fn empty_record() -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::new(),
    })
}
