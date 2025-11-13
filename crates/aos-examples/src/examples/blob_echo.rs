use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use aos_air_types::{
    CapGrant, CapType, DefCap, DefModule, DefPolicy, DefSchema, EffectKind as AirEffectKind,
    HashRef, Manifest, ManifestDefaults, ModuleAbi, ModuleBinding, ModuleKind, Name, NamedRef,
    PolicyDecision, PolicyMatch, PolicyRule, ReducerAbi, Routing, RoutingEvent, SchemaRef,
    TypeExpr, TypeOption, TypePrimitive, TypePrimitiveBytes, TypePrimitiveText, TypePrimitiveUnit,
    TypeRecord, TypeRef, TypeVariant, ValueLiteral, ValueRecord, builtins,
};
use aos_cbor::Hash;
use aos_effects::builtins::{BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt};
use aos_effects::{EffectIntent, EffectKind, EffectReceipt, ReceiptStatus};
use aos_kernel::journal::fs::FsJournal;
use aos_kernel::{Kernel, LoadedManifest};
use aos_store::{FsStore, Store};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::examples::util;

const REDUCER_NAME: &str = "demo/BlobEchoSM@1";
const STATE_SCHEMA: &str = "demo/BlobEchoState@1";
const EVENT_SCHEMA: &str = "demo/BlobEchoEvent@1";
const PC_SCHEMA: &str = "demo/BlobEchoPc@1";
const CAP_NAME: &str = "sys/blob@1";
const POLICY_NAME: &str = "demo/blob_policy@1";
const CAP_GRANT: &str = "blob_grant";
const ADAPTER_ID: &str = "adapter.blob.fake";

const SYS_BLOB_PUT_RESULT: &str = "sys/BlobPutResult@1";
const SYS_BLOB_GET_RESULT: &str = "sys/BlobGetResult@1";
const SYS_BLOB_PUT_PARAMS: &str = "sys/BlobPutParams@1";
const SYS_BLOB_PUT_RECEIPT: &str = "sys/BlobPutReceipt@1";
const SYS_BLOB_GET_PARAMS: &str = "sys/BlobGetParams@1";
const SYS_BLOB_GET_RECEIPT: &str = "sys/BlobGetReceipt@1";

#[derive(Debug, Clone)]
struct BlobEchoInput {
    namespace: String,
    key: String,
    data: Vec<u8>,
}

#[derive(Default)]
struct BlobHarnessStore {
    pending_blobs: HashMap<String, Vec<u8>>,
    key_to_blob: HashMap<(String, String), String>,
}

pub fn run(example_root: &Path) -> Result<()> {
    util::reset_journal(example_root)?;
    let wasm_bytes = util::ensure_wasm_artifact(
        "examples/02-blob-echo/reducer/Cargo.toml",
        "target/wasm32-unknown-unknown/debug/blob_echo_reducer.wasm",
        "blob-echo",
    )?;
    let store = Arc::new(FsStore::open(example_root).context("open FsStore")?);
    let loaded = build_loaded_manifest(store.clone(), &wasm_bytes).context("build manifest")?;
    let journal = Box::new(FsJournal::open(example_root)?);
    let mut kernel = Kernel::from_loaded_manifest(store.clone(), loaded, journal)?;

    let input = BlobEchoInput {
        namespace: "demo".into(),
        key: "echo".into(),
        data: b"Blob Echo Example".to_vec(),
    };
    println!("→ Blob Echo demo");
    drive_blob_echo(&mut kernel, &input).context("drive blob echo")?;

    let final_state = read_state(&kernel).context("read final state")?;
    println!(
        "   final state: pc={:?}, stored_ref={:?}, retrieved_ref={:?}",
        final_state.pc, final_state.stored_blob_ref, final_state.retrieved_blob_ref
    );

    drop(kernel);

    let replay_loaded = build_loaded_manifest(store.clone(), &wasm_bytes)?;
    let replay_journal = Box::new(FsJournal::open(example_root)?);
    let replay = Kernel::from_loaded_manifest(store, replay_loaded, replay_journal)?;
    let replay_state = read_state(&replay).context("read replay state")?;
    if replay_state.stored_blob_ref != final_state.stored_blob_ref
        || replay_state.retrieved_blob_ref != final_state.retrieved_blob_ref
    {
        return Err(anyhow!("replay mismatch: blob refs diverged"));
    }
    let state_hash = Hash::of_cbor(&final_state)?.to_hex();
    println!("   replay check: OK (state hash {state_hash})\n");

    Ok(())
}

fn drive_blob_echo(kernel: &mut Kernel<FsStore>, input: &BlobEchoInput) -> Result<()> {
    let mut harness = BlobHarnessStore::default();
    let blob_ref = hash_bytes(&input.data);
    harness
        .pending_blobs
        .insert(blob_ref.clone(), input.data.clone());
    harness
        .key_to_blob
        .insert((input.namespace.clone(), input.key.clone()), blob_ref);

    let start_event = StartEvent {
        namespace: input.namespace.clone(),
        key: input.key.clone(),
        data: input.data.clone(),
    };
    submit_start(kernel, &start_event).context("submit start")?;
    synthesize_blob_effects(kernel, &mut harness).context("drain blob effects")?;
    Ok(())
}

fn submit_start(kernel: &mut Kernel<FsStore>, event: &StartEvent) -> Result<()> {
    let payload = serde_cbor::to_vec(event)?;
    kernel.submit_domain_event(EVENT_SCHEMA, payload);
    kernel.tick_until_idle()?;
    Ok(())
}

fn synthesize_blob_effects(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
) -> Result<()> {
    loop {
        let intents = kernel.drain_effects();
        if intents.is_empty() {
            break;
        }
        for intent in intents {
            match intent.kind.as_str() {
                EffectKind::BLOB_PUT => handle_blob_put(kernel, harness, intent)?,
                EffectKind::BLOB_GET => handle_blob_get(kernel, harness, intent)?,
                other => return Err(anyhow!("unexpected effect {other}")),
            }
        }
    }
    Ok(())
}

fn handle_blob_put(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
    intent: EffectIntent,
) -> Result<()> {
    let params: BlobPutParams = serde_cbor::from_slice(&intent.params_cbor)?;
    let blob_ref = params.blob_ref.as_str().to_string();
    let data = harness
        .pending_blobs
        .get(&blob_ref)
        .ok_or_else(|| anyhow!("missing blob data for {blob_ref}"))?;
    println!(
        "     blob.put -> namespace={} blob_ref={} size={} bytes",
        params.namespace,
        blob_ref,
        data.len()
    );

    let receipt_payload = BlobPutReceipt {
        blob_ref: params.blob_ref.clone(),
        size: data.len() as u64,
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
    Ok(())
}

fn handle_blob_get(
    kernel: &mut Kernel<FsStore>,
    harness: &mut BlobHarnessStore,
    intent: EffectIntent,
) -> Result<()> {
    let params: BlobGetParams = serde_cbor::from_slice(&intent.params_cbor)?;
    let key = (params.namespace.clone(), params.key.clone());
    let blob_ref = harness.key_to_blob.get(&key).ok_or_else(|| {
        anyhow!(
            "no blob stored for namespace={} key={}",
            params.namespace,
            params.key
        )
    })?;
    let data = harness
        .pending_blobs
        .get(blob_ref)
        .ok_or_else(|| anyhow!("missing blob bytes for {blob_ref}"))?;
    println!(
        "     blob.get -> namespace={} key={} size={} bytes",
        params.namespace,
        params.key,
        data.len()
    );

    let receipt_payload = BlobGetReceipt {
        blob_ref: HashRef::new(blob_ref.clone()).map_err(|err| anyhow!("invalid hash: {err}"))?,
        size: data.len() as u64,
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
    Ok(())
}

fn read_state(kernel: &Kernel<FsStore>) -> Result<ReducerEchoState> {
    let bytes = kernel
        .reducer_state(REDUCER_NAME)
        .cloned()
        .ok_or_else(|| anyhow!("missing state for {REDUCER_NAME}"))?;
    Ok(serde_cbor::from_slice(&bytes)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReducerEchoState {
    pc: ReducerPc,
    stored_blob_ref: Option<String>,
    retrieved_blob_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ReducerPc {
    Idle,
    Putting,
    Getting,
    Done,
}

fn build_loaded_manifest(store: Arc<FsStore>, wasm_bytes: &[u8]) -> Result<LoadedManifest> {
    let pc_schema = blob_pc_schema();
    let state_schema = blob_state_schema();
    let event_schema = blob_event_schema()?;
    let cap = blob_cap_schema();
    let policy = blob_policy();
    let wasm_hash = store.put_blob(wasm_bytes)?;
    let module = blob_module(wasm_hash)?;

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
    for builtin in [
        SYS_BLOB_PUT_PARAMS,
        SYS_BLOB_PUT_RECEIPT,
        SYS_BLOB_GET_PARAMS,
        SYS_BLOB_GET_RECEIPT,
        SYS_BLOB_PUT_RESULT,
        SYS_BLOB_GET_RESULT,
    ] {
        if let Some(node) = builtins::find_builtin_schema(builtin) {
            schemas.insert(builtin.into(), node.schema.clone());
        }
    }

    let manifest = Manifest {
        schemas: vec![
            named_ref(PC_SCHEMA, pc_hash)?,
            named_ref(STATE_SCHEMA, state_hash)?,
            named_ref(EVENT_SCHEMA, event_hash)?,
            builtin_named_ref(SYS_BLOB_PUT_PARAMS)?,
            builtin_named_ref(SYS_BLOB_PUT_RECEIPT)?,
            builtin_named_ref(SYS_BLOB_GET_PARAMS)?,
            builtin_named_ref(SYS_BLOB_GET_RECEIPT)?,
            builtin_named_ref(SYS_BLOB_PUT_RESULT)?,
            builtin_named_ref(SYS_BLOB_GET_RESULT)?,
        ],
        modules: vec![named_ref(REDUCER_NAME, module_hash)?],
        plans: Vec::new(),
        caps: vec![named_ref(CAP_NAME, cap_hash)?],
        policies: vec![named_ref(POLICY_NAME, policy_hash)?],
        defaults: Some(ManifestDefaults {
            policy: Some(POLICY_NAME.into()),
            cap_grants: vec![CapGrant {
                name: CAP_GRANT.into(),
                cap: CAP_NAME.into(),
                params: empty_record(),
                expiry_ns: None,
                budget: None,
            }],
        }),
        module_bindings: {
            let mut bindings = IndexMap::new();
            let mut slots = IndexMap::new();
            slots.insert("blob".into(), CAP_GRANT.into());
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
                    event: schema_ref(SYS_BLOB_PUT_RESULT)?,
                    reducer: REDUCER_NAME.into(),
                    key_field: None,
                },
                RoutingEvent {
                    event: schema_ref(SYS_BLOB_GET_RESULT)?,
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

fn blob_pc_schema() -> DefSchema {
    let mut variants = IndexMap::new();
    variants.insert("Idle".into(), unit_type());
    variants.insert("Putting".into(), unit_type());
    variants.insert("Getting".into(), unit_type());
    variants.insert("Done".into(), unit_type());
    DefSchema {
        name: PC_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant: variants }),
    }
}

fn blob_state_schema() -> DefSchema {
    let mut fields = IndexMap::new();
    fields.insert(
        "pc".into(),
        TypeExpr::Ref(TypeRef {
            reference: schema_ref(PC_SCHEMA).expect("pc ref"),
        }),
    );
    fields.insert("namespace".into(), option_text());
    fields.insert("key".into(), option_text());
    fields.insert("stored_blob_ref".into(), option_text());
    fields.insert("retrieved_blob_ref".into(), option_text());
    DefSchema {
        name: STATE_SCHEMA.into(),
        ty: TypeExpr::Record(TypeRecord { record: fields }),
    }
}

fn blob_event_schema() -> Result<DefSchema> {
    let mut variants = IndexMap::new();
    let mut start_record = IndexMap::new();
    start_record.insert("namespace".into(), text_type());
    start_record.insert("key".into(), text_type());
    start_record.insert("data".into(), bytes_type());
    variants.insert(
        "Start".into(),
        TypeExpr::Record(TypeRecord {
            record: start_record,
        }),
    );
    variants.insert(
        "PutResult".into(),
        TypeExpr::Ref(TypeRef {
            reference: schema_ref(SYS_BLOB_PUT_RESULT)?,
        }),
    );
    variants.insert(
        "GetResult".into(),
        TypeExpr::Ref(TypeRef {
            reference: schema_ref(SYS_BLOB_GET_RESULT)?,
        }),
    );
    Ok(DefSchema {
        name: EVENT_SCHEMA.into(),
        ty: TypeExpr::Variant(TypeVariant { variant: variants }),
    })
}

fn blob_module(wasm_hash: Hash) -> Result<DefModule> {
    let mut cap_slots = IndexMap::new();
    cap_slots.insert("blob".into(), CapType::Blob);
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
                effects_emitted: vec![AirEffectKind::BlobPut, AirEffectKind::BlobGet],
                cap_slots,
            }),
        },
    })
}

fn blob_cap_schema() -> DefCap {
    DefCap {
        name: CAP_NAME.into(),
        cap_type: CapType::Blob,
        schema: TypeExpr::Record(TypeRecord {
            record: IndexMap::new(),
        }),
    }
}

fn blob_policy() -> DefPolicy {
    DefPolicy {
        name: POLICY_NAME.into(),
        rules: vec![
            PolicyRule {
                when: PolicyMatch {
                    effect_kind: Some(AirEffectKind::BlobPut),
                    origin_kind: Some(aos_air_types::OriginKind::Reducer),
                    ..Default::default()
                },
                decision: PolicyDecision::Allow,
            },
            PolicyRule {
                when: PolicyMatch {
                    effect_kind: Some(AirEffectKind::BlobGet),
                    origin_kind: Some(aos_air_types::OriginKind::Reducer),
                    ..Default::default()
                },
                decision: PolicyDecision::Allow,
            },
        ],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    namespace: String,
    key: String,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

fn text_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
        text: Default::default(),
    }))
}

fn bytes_type() -> TypeExpr {
    TypeExpr::Primitive(TypePrimitive::Bytes(TypePrimitiveBytes {
        bytes: Default::default(),
    }))
}

fn option_text() -> TypeExpr {
    TypeExpr::Option(TypeOption {
        option: Box::new(text_type()),
    })
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

fn empty_record() -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: IndexMap::new(),
    })
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}
