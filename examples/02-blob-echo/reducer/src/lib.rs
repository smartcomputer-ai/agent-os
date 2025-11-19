#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::{format, string::{String, ToString}, vec::Vec};
use aos_air_types::HashRef;
use aos_effects::builtins::{BlobGetParams, BlobGetReceipt, BlobPutParams, BlobPutReceipt};
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx, Value};
use serde::{Deserialize, Serialize};
use serde_bytes;
use sha2::{Digest, Sha256};

const SYS_BLOB_PUT_RESULT: &str = "sys/BlobPutResult@1";
const SYS_BLOB_GET_RESULT: &str = "sys/BlobGetResult@1";

aos_reducer!(BlobEchoSm);

#[derive(Default)]
struct BlobEchoSm;

impl Reducer for BlobEchoSm {
    type State = EchoState;
    type Event = BlobEchoEvent;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State>,
    ) -> Result<(), ReduceError> {
        match event {
            BlobEchoEvent::Start(start) => handle_start(ctx, start),
            BlobEchoEvent::PutResult(result) => handle_put_result(ctx, result),
            BlobEchoEvent::GetResult(result) => handle_get_result(ctx, result),
        }
        Ok(())
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum BlobEchoEvent {
    Start(StartEvent),
    PutResult(BlobPutResultEvent),
    GetResult(BlobGetResultEvent),
}

fn handle_start(ctx: &mut ReducerCtx<EchoState>, event: StartEvent) {
    if !matches!(ctx.state.pc, EchoPc::Idle | EchoPc::Done) {
        return;
    }
    let blob_ref = hash_bytes(&event.data);
    ctx.state.pc = EchoPc::Putting;
    ctx.state.namespace = Some(event.namespace.clone());
    ctx.state.key = Some(event.key.clone());
    ctx.state.pending_blob_ref = Some(blob_ref.clone());

    let params = BlobPutParams {
        namespace: event.namespace,
        blob_ref: HashRef::new(blob_ref).expect("blob hash"),
    };
    ctx.effects().emit_raw("blob.put", &params, Some("blob"));
}

fn handle_put_result(ctx: &mut ReducerCtx<EchoState>, event: BlobPutResultEvent) {
    if !matches!(ctx.state.pc, EchoPc::Putting) {
        return;
    }
    if event.status != "ok" {
        ctx.state.pc = EchoPc::Done;
        return;
    }
    if let Some(expected) = &ctx.state.pending_blob_ref {
        if event.receipt.blob_ref.as_str() != expected {
            ctx.state.pc = EchoPc::Done;
            return;
        }
        ctx.state.stored_blob_ref = Some(expected.clone());
    }
    ctx.state.pc = EchoPc::Getting;
    if let (Some(namespace), Some(key)) = (&ctx.state.namespace, &ctx.state.key) {
        let params = BlobGetParams {
            namespace: namespace.clone(),
            key: key.clone(),
        };
        ctx.effects().emit_raw("blob.get", &params, Some("blob"));
    }
}

fn handle_get_result(ctx: &mut ReducerCtx<EchoState>, event: BlobGetResultEvent) {
    if !matches!(ctx.state.pc, EchoPc::Getting) {
        return;
    }
    if event.status == "ok" {
        ctx.state.retrieved_blob_ref = Some(event.receipt.blob_ref.as_str().to_string());
    }
    ctx.state.pc = EchoPc::Done;
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("sha256:{}", hex::encode(digest))
}
