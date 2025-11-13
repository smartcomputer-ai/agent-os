#![allow(improper_ctypes_definitions)]

use aos_air_types::HashRef;
use aos_effects::builtins::{BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt};
use aos_wasm_abi::{ReducerEffect, ReducerInput, ReducerOutput};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::alloc::{Layout, alloc as host_alloc};
use std::slice;

const EVENT_SCHEMA: &str = "demo/BlobEchoEvent@1";
const SYS_BLOB_PUT_RESULT: &str = "sys/BlobPutResult@1";
const SYS_BLOB_GET_RESULT: &str = "sys/BlobGetResult@1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct EchoState {
    pc: EchoPc,
    namespace: Option<String>,
    key: Option<String>,
    pending_blob_ref: Option<String>,
    stored_blob_ref: Option<String>,
    retrieved_blob_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum EchoPc {
    Idle,
    Putting,
    Getting,
    Done,
}

impl Default for EchoPc {
    fn default() -> Self {
        EchoPc::Idle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartEvent {
    namespace: String,
    key: String,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobPutResultEvent {
    status: String,
    requested: BlobPutParams,
    receipt: BlobPutReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlobGetResultEvent {
    status: String,
    requested: BlobGetParams,
    receipt: BlobGetReceipt,
}

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "alloc"))]
pub extern "C" fn wasm_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    let layout = Layout::from_size_align(len as usize, 8).expect("layout");
    unsafe { host_alloc(layout) as i32 }
}

#[cfg_attr(target_arch = "wasm32", unsafe(export_name = "step"))]
pub extern "C" fn wasm_step(ptr: i32, len: i32) -> (i32, i32) {
    let input_bytes = unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) };
    let input = ReducerInput::decode(input_bytes).expect("valid reducer input");

    let mut state = input
        .state
        .map(|bytes| serde_cbor::from_slice::<EchoState>(&bytes).expect("state"))
        .unwrap_or_default();
    let mut effects = Vec::new();

    match input.event.schema.as_str() {
        EVENT_SCHEMA => {
            if let Ok(event) = serde_cbor::from_slice::<StartEvent>(&input.event.value) {
                handle_start(&mut state, event, &mut effects);
            } else {
                // Unknown variant, ignore
            }
        }
        SYS_BLOB_PUT_RESULT => {
            if let Ok(event) = serde_cbor::from_slice::<BlobPutResultEvent>(&input.event.value) {
                handle_put_result(&mut state, event, &mut effects);
            }
        }
        SYS_BLOB_GET_RESULT => {
            if let Ok(event) = serde_cbor::from_slice::<BlobGetResultEvent>(&input.event.value) {
                handle_get_result(&mut state, event);
            }
        }
        _ => {}
    }

    let state_bytes = serde_cbor::to_vec(&state).expect("encode state");
    let output = ReducerOutput {
        state: Some(state_bytes),
        domain_events: Vec::new(),
        effects,
        ann: None,
    };
    let output_bytes = output.encode().expect("encode output");
    write_back(&output_bytes)
}

fn handle_start(state: &mut EchoState, event: StartEvent, effects: &mut Vec<ReducerEffect>) {
    if !matches!(state.pc, EchoPc::Idle | EchoPc::Done) {
        return;
    }
    let blob_ref = hash_bytes(&event.data);
    state.pc = EchoPc::Putting;
    state.namespace = Some(event.namespace.clone());
    state.key = Some(event.key.clone());
    state.pending_blob_ref = Some(blob_ref.clone());
    emit_blob_put(effects, &event.namespace, &blob_ref);
}

fn handle_put_result(
    state: &mut EchoState,
    event: BlobPutResultEvent,
    effects: &mut Vec<ReducerEffect>,
) {
    if !matches!(state.pc, EchoPc::Putting) {
        return;
    }
    if event.status != "ok" {
        state.pc = EchoPc::Done;
        return;
    }
    if let Some(expected) = &state.pending_blob_ref {
        if event.receipt.blob_ref.as_str() != expected {
            state.pc = EchoPc::Done;
            return;
        }
        state.stored_blob_ref = Some(expected.clone());
    }
    state.pc = EchoPc::Getting;
    if let (Some(namespace), Some(key)) = (&state.namespace, &state.key) {
        emit_blob_get(effects, namespace, key);
    }
}

fn handle_get_result(state: &mut EchoState, event: BlobGetResultEvent) {
    if !matches!(state.pc, EchoPc::Getting) {
        return;
    }
    if event.status == "ok" {
        state.retrieved_blob_ref = Some(event.receipt.blob_ref.as_str().to_string());
    }
    state.pc = EchoPc::Done;
}

fn emit_blob_put(effects: &mut Vec<ReducerEffect>, namespace: &str, blob_ref: &str) {
    let params = BlobPutParams {
        namespace: namespace.into(),
        blob_ref: HashRef::new(blob_ref.to_string()).expect("hash ref"),
    };
    let params_bytes = serde_cbor::to_vec(&params).expect("params");
    effects.push(ReducerEffect::with_cap_slot(
        "blob.put",
        params_bytes,
        "blob",
    ));
}

fn emit_blob_get(effects: &mut Vec<ReducerEffect>, namespace: &str, key: &str) {
    let params = BlobGetParams {
        namespace: namespace.into(),
        key: key.into(),
    };
    let params_bytes = serde_cbor::to_vec(&params).expect("params");
    effects.push(ReducerEffect::with_cap_slot(
        "blob.get",
        params_bytes,
        "blob",
    ));
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len() as i32;
    let ptr = wasm_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        out.copy_from_slice(bytes);
    }
    (ptr, len)
}
